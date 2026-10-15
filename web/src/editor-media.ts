// Redaktion: images for finds, zones and chapters (server: editor_media.rs).
// Upload or pick a Commons suggestion; a second person approves (the server
// refuses the uploader). Only approved images reach the app.

import { ask, COLL_NAME, errorOf, setBadge, toast } from "./editor-ui";
import { t } from "./editor-i18n";

type Api = (path: string, init?: RequestInit) => Promise<Response>;
type Kind = "find" | "zone" | "challenge";
type Image = {
  id: number;
  kind: Kind;
  target: string;
  url: string | null;
  width: number;
  height: number;
  author: string;
  licence: string;
  licence_label: string | null;
  licence_url: string | null;
  source_url: string | null;
  alt: string | null;
  status: "pending" | "approved" | "rejected";
  by: string;
  approved_by: string | null;
  created: number;
  bytes: number;
};
type Suggestion = { title: string; thumb: string; page: string; author: string; licence: string; licence_url: string | null; distance_m: number };
type Target = { id: string; name: string; collection?: string; hidden?: boolean; focus?: boolean };
type Data = {
  store: boolean;
  images: Image[];
  suggestions: Record<string, Suggestion[]>;
  targets: Record<Kind, Target[]>;
  limits: { bytes: number; side: number };
};

const KIND_NAME = (k: Kind) => t(`me.k.${k}`);
const STATUS_NAME = (s: Image["status"]) => t(`me.st.${s}`);

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}, ...kids: (Node | string)[]) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...kids);
  return e;
};

let api: Api;
let region = "";
let data: Data | null = null;

const q = () => `region=${encodeURIComponent(region)}`;
const kind = () => $<HTMLSelectElement>("media-kind").value as Kind;
const target = () => $<HTMLSelectElement>("media-target").value;
const targetName = (k: Kind, id: string) => data?.targets[k].find((t) => t.id === id)?.name ?? id;
const error = errorOf;

/** "Foto: author, licence" with links, as in the app. */
function credit(i: { author: string; licence_label: string | null; licence_url: string | null; source_url: string | null }) {
  const p = el("small", { className: "media-credit" }, t("me.photo"));
  p.append(i.source_url ? el("a", { href: i.source_url, target: "_blank", rel: "noopener noreferrer", textContent: i.author }) : i.author);
  if (i.licence_label) {
    p.append(", ", i.licence_url ? el("a", { href: i.licence_url, target: "_blank", rel: "noopener noreferrer", textContent: i.licence_label }) : i.licence_label);
  }
  return p;
}

function thumb(url: string | null, alt: string) {
  return url ? el("img", { src: url, alt, loading: "lazy", className: "media-thumb" }) : el("div", { className: "media-thumb empty", textContent: t("me.noimg") });
}

async function decide(i: Image, approve: boolean) {
  const name = t("me.name", { kind: KIND_NAME(i.kind), name: targetName(i.kind, i.target) });
  const old = data?.images.some((x) => x.kind === i.kind && x.target === i.target && x.status === "approved");
  if (approve && old && !(await ask(t("me.replace.ask", { name }), t("me.replace.yes")))) return;
  if (!approve && !(await ask(t("me.reject.ask", { name }), t("me.reject"), true))) return;
  const res = await api(`/api/ops/media/decide?${q()}`, { method: "POST", body: JSON.stringify({ id: i.id, approve }) }).catch(() => null);
  if (!res?.ok) toast(await error(res, t(approve ? "ui.approvefail" : "me.rejectfail")), "error");
  else toast(approve ? t("me.approved", { name }) : t("me.rejected"));
  await loadMedia(region);
}

async function remove(i: Image) {
  if (!(await ask(t(i.status === "approved" ? "me.delete.live" : "me.delete.ask"), t("ui.delete"), true))) return;
  const res = await api(`/api/ops/media?${q()}&id=${i.id}`, { method: "DELETE" }).catch(() => null);
  if (!res?.ok) toast(await error(res, t("ui.deletefail")), "error");
  else toast(t("me.deleted"));
  await loadMedia(region);
}

/** One image with its state and buttons. */
function card(i: Image, withTarget: boolean) {
  const box = el("div", { className: `media-item ${i.status}` });
  box.append(thumb(i.url, i.alt ?? ""));
  const info = el("div");
  if (withTarget) info.append(el("b", { textContent: `${KIND_NAME(i.kind)}: ${targetName(i.kind, i.target)}` }), el("br"));
  info.append(el("span", { className: "st", textContent: STATUS_NAME(i.status) }), el("br"), credit(i), el("br"));
  const by = i.approved_by ? t(i.status === "rejected" ? "me.meta.rejected" : "me.meta.approved", { by: i.approved_by }) : "";
  info.append(el("small", { textContent: t("me.meta", { w: i.width, h: i.height, kb: Math.round(i.bytes / 1000), by: i.by }) + by }));
  if (i.alt) info.append(el("br"), el("small", { textContent: t("me.alt.show", { alt: i.alt }) }));
  const row = el("div", { className: "ed-row" });
  if (i.status === "pending") {
    row.append(el("button", { type: "button", className: "primary", textContent: t("ui.approve"), title: t("me.approve.title"), onclick: () => void decide(i, true) }));
    row.append(el("button", { type: "button", textContent: t("me.reject"), onclick: () => void decide(i, false) }));
  }
  row.append(el("button", { type: "button", textContent: t("ui.delete"), onclick: () => void remove(i) }));
  info.append(row);
  box.append(info);
  return box;
}

function drawPending() {
  const pending = data!.images.filter((i) => i.status === "pending");
  $("media-pending-count").textContent = pending.length ? `(${pending.length})` : "";
  setBadge("bilder", pending.length ? String(pending.length) : "");
  const host = $("media-pending");
  host.className = pending.length ? "" : "empty";
  if (!pending.length) host.textContent = t("me.nothing.pending");
  else
    host.replaceChildren(
      ...pending.map((i) => {
        // Next to the one it would replace.
        const now = data!.images.find((x) => x.kind === i.kind && x.target === i.target && x.status === "approved");
        if (!now) return card(i, true);
        const pair = el("div", { className: "media-pair" });
        const was = el("div", { className: "media-was" }, el("small", { textContent: t("me.was") }), thumb(now.url, now.alt ?? ""), credit(now));
        pair.append(card(i, true), was);
        return pair;
      }),
    );
}

function drawTargets() {
  const k = kind();
  const before = target();
  const only = k === "find" && input("media-only").checked;
  $("media-only-label").hidden = k !== "find";
  const img = (id: string) => data!.images.find((i) => i.kind === k && i.target === id && i.status === "approved");
  const words = input("media-search").value.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const list = data!.targets[k].filter(
    (t) => (!only || data!.suggestions[t.id]?.length) && words.every((w) => t.name.toLowerCase().includes(w)),
  );
  $("media-search-count").textContent = words.length || only ? t("me.count", { n: list.length, m: data!.targets[k].length }) : "";
  const sel = $<HTMLSelectElement>("media-target");
  sel.replaceChildren(
    ...list.map((x) => {
      const n = k === "find" ? data!.suggestions[x.id]?.length ?? 0 : 0;
      const parts = [x.name];
      if (x.collection) parts.push(`(${COLL_NAME[x.collection] ?? x.collection}${x.hidden ? t("me.t.hidden") : ""})`);
      if (x.focus) parts.push(t("me.t.focus"));
      if (img(x.id)) parts.push(t("me.t.img"));
      if (n) parts.push(t("me.t.sugg", { n }));
      return new Option(parts.join(" "), x.id);
    }),
  );
  if (list.some((t) => t.id === before)) sel.value = before;
  drawTarget();
}

function drawTarget() {
  const k = kind(), id = target();
  const cur = $("media-current");
  if (!id) {
    cur.replaceChildren(el("p", { className: "caption", textContent: t(k === "challenge" ? "me.no.chapters" : "me.nothing") }));
    $("media-suggest-box").hidden = true;
    return;
  }
  const mine = data!.images.filter((i) => i.kind === k && i.target === id);
  cur.replaceChildren(...(mine.length ? mine.map((i) => card(i, false)) : [el("p", { className: "caption", textContent: t("me.none") })]));
  const sugg = k === "find" ? data!.suggestions[id] ?? [] : [];
  $("media-suggest-box").hidden = !sugg.length;
  $("media-suggest").replaceChildren(
    ...sugg.map((s) => {
      const b = el("div", { className: "media-sugg" });
      b.append(
        el("a", { href: s.page, target: "_blank", rel: "noopener noreferrer" }, el("img", { src: s.thumb, alt: s.title, loading: "lazy", className: "media-thumb" })),
        el("small", { textContent: `${s.author} · ${s.licence} · ${Math.round(s.distance_m)} m` }),
        altInput(targetName("find", id)),
        el("button", { type: "button", textContent: t("me.take"), disabled: !data!.store, onclick: () => void pick(id, s, b) }),
      );
      return b;
    }),
  );
}

function altInput(value: string): HTMLInputElement {
  const i = document.createElement("input");
  Object.assign(i, { className: "sugg-alt", maxLength: 200, value, title: t("me.alt.title"), placeholder: t("me.alt") });
  return i;
}

async function pick(find: string, s: Suggestion, box: HTMLElement) {
  const btn = box.querySelector("button")!;
  btn.disabled = true;
  btn.textContent = t("me.loading");
  const alt = box.querySelector<HTMLInputElement>(".sugg-alt")?.value.trim() || targetName("find", find);
  const res = await api(`/api/ops/media/commons?${q()}`, { method: "POST", body: JSON.stringify({ find, title: s.title, alt }) }).catch(() => null);
  if (!res?.ok) {
    toast(await error(res, t("me.takefail")), "error");
    btn.disabled = false;
    btn.textContent = t("me.take");
    return;
  }
  toast(t("me.taken"));
  await loadMedia(region);
}

function readFile(f: File): Promise<string> {
  return new Promise((ok, fail) => {
    const r = new FileReader();
    r.onload = () => ok(String(r.result));
    r.onerror = () => fail(r.error);
    r.readAsDataURL(f);
  });
}

async function upload() {
  const err = $("media-error");
  err.textContent = "";
  const f = input("media-file").files?.[0];
  if (!f) return void (err.textContent = t("me.e.file"));
  if (f.size > (data?.limits.bytes ?? 8 << 20)) return void (err.textContent = t("me.e.big"));
  const author = input("media-author").value.trim();
  if (!author) return void (err.textContent = t("me.e.author"));
  if (!target()) return void (err.textContent = t("me.e.target"));
  const btn = $<HTMLButtonElement>("media-upload");
  btn.disabled = true;
  const body = {
    kind: kind(),
    target: target(),
    data: await readFile(f),
    author,
    licence: $<HTMLSelectElement>("media-licence").value,
    alt: input("media-alt").value.trim(),
    source_url: input("media-source").value.trim() || null,
  };
  const res = await api(`/api/ops/media?${q()}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  btn.disabled = false;
  if (!res?.ok) return void (err.textContent = await error(res, t("me.uploadfail")));
  input("media-file").value = "";
  input("media-alt").value = "";
  input("media-source").value = "";
  toast(t("me.uploaded"));
  await loadMedia(region);
}

export function initMedia(apiFn: Api) {
  api = apiFn;
  $("media-kind").onchange = drawTargets;
  input("media-only").onchange = drawTargets;
  $("media-target").onchange = drawTarget;
  input("media-search").oninput = drawTargets;
  $("media-upload").onclick = () => void upload();
}

export async function loadMedia(slug: string) {
  region = slug;
  const res = await api(`/api/ops/media?${q()}`).catch(() => null);
  $("media-card").hidden = !res?.ok;
  if (!res?.ok) return;
  data = (await res.json()) as Data;
  $("media-off").hidden = data.store;
  $<HTMLButtonElement>("media-upload").disabled = !data.store;
  drawPending();
  drawTargets();
}
