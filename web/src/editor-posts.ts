// Redaktion: posts from the game (server: posts.rs). A post has a trigger (a
// time, a chapter's start or end, or by hand), texts per outcome and channel,
// and maybe the picture of the game state. When it is due the server makes a
// draft with the numbers filled in; here it is copied, posted by hand and
// marked, or sent from here to the channels the server has an account for.
import { ask, clean, errorOf, fmtDate, fromLocal, mayLeave, toast, toLocal, watchForm } from "./editor-ui";
import { t, type EKey } from "./editor-i18n";

type Texts = { instagram?: string; mastodon?: string; alt?: string };
type Variant = "any" | "fire" | "water" | "tie";
type Card = { lat?: number; lon?: number; width_m: number; title: string; note: string };
type Post = {
  id: number;
  title: string;
  trigger: "at" | "chapter_start" | "chapter_end" | "manual";
  at: number | null;
  chapter: string | null;
  offset_min: number;
  texts: Partial<Record<Variant, Texts>>;
  card: Card | null;
  channels: string[];
  by: string;
};
type Run = {
  id: number;
  post: number;
  auto: boolean;
  created: number;
  state: "draft" | "sent" | "discarded";
  outcome: "fire" | "water" | "tie" | null;
  numbers: { fire: number; water: number } | null;
  texts: Texts;
  image: boolean;
  note: string | null;
  decided_by: string | null;
  decided: number | null;
  results: Record<string, { url?: string; error?: string }> | null;
};
export type ChapterRef = { key: string; names: Record<string, { title: string }>; starts: number; ends: number };
type Api = (path: string, init?: RequestInit) => Promise<Response>;

const VARIANTS: Variant[] = ["any", "fire", "water", "tie"];
const FIELDS = ["instagram", "mastodon", "alt"] as const;
const MAX = { instagram: 2200, mastodon: 500, alt: 1000 };
const LABEL: Record<string, EKey> = { instagram: "po.f.instagram", mastodon: "po.f.mastodon", alt: "po.f.alt" };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);
const area = (id: string) => $<HTMLTextAreaElement>(id);
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, props: Partial<HTMLElementTagNameMap[K]> = {}) => Object.assign(document.createElement(tag), props);

let api: Api;
let region = "";
let primary = "de";
let chapters: ChapterRef[] = [];
let posts: Post[] = [];
let runs: Run[] = [];
/** Channels the server can send to for this region. */
let ready: string[] = [];
let editing: Post | null = null;
/** The form's texts per variant (the shown one lives in the text areas). */
let texts: Partial<Record<Variant, Texts>> = {};
let shown: Variant = "any";
const pictures = new Map<number, string>();

const q = () => `region=${encodeURIComponent(region)}`;
const chapterName = (key: string | null) => {
  const c = chapters.find((x) => x.key === key);
  return c ? (c.names[primary]?.title ?? Object.values(c.names)[0]?.title ?? key!) : (key ?? "");
};

function keepShown() {
  const v: Texts = {};
  for (const f of FIELDS) {
    const x = area(`post-${f}`).value.trim();
    if (x) v[f] = x;
  }
  texts[shown] = v;
}

function showVariant(v: Variant) {
  keepShown();
  shown = v;
  for (const f of FIELDS) area(`post-${f}`).value = texts[v]?.[f] ?? "";
  for (const b of document.querySelectorAll<HTMLButtonElement>("#post-variants button")) b.classList.toggle("on", b.dataset.v === v);
  counts();
}

function counts() {
  for (const f of FIELDS) $(`post-${f}-count`).textContent = `${area(`post-${f}`).value.length} / ${MAX[f]}`;
  for (const b of document.querySelectorAll<HTMLButtonElement>("#post-variants button")) {
    const v = b.dataset.v as Variant;
    const has = v === shown ? FIELDS.some((f) => area(`post-${f}`).value.trim()) : !!(texts[v]?.instagram || texts[v]?.mastodon);
    b.classList.toggle("has", has);
  }
}

function syncTrigger() {
  const tr = $<HTMLSelectElement>("post-trigger").value;
  // style, not `hidden`: the form's label and row rules would win.
  $("post-at-row").style.display = tr === "at" ? "" : "none";
  $("post-chapter-row").style.display = tr.startsWith("chapter_") ? "" : "none";
  $("post-card-fields").style.display = input("post-card-on").checked ? "" : "none";
}

function open(p: Post | null) {
  clean("post-form");
  editing = p;
  $("post-form-title").textContent = t(p ? "po.edit" : "po.new");
  input("post-name").value = p?.title ?? "";
  $<HTMLSelectElement>("post-trigger").value = p?.trigger ?? "chapter_end";
  const sel = $<HTMLSelectElement>("post-chapter");
  sel.replaceChildren(...chapters.map((c) => new Option(`${chapterName(c.key)} (${fmtDate(c.starts, false)})`, c.key)));
  if (p?.chapter) sel.value = p.chapter;
  input("post-offset").value = String(p?.offset_min ?? 0);
  input("post-at").value = toLocal(p?.at ?? Math.floor(Date.now() / 3_600_000 + 1) * 3600);
  for (const c of ["instagram", "mastodon"]) input(`post-ch-${c}`).checked = p ? p.channels.includes(c) : true;
  texts = structuredClone(p?.texts ?? {});
  shown = "any";
  for (const f of FIELDS) area(`post-${f}`).value = texts.any?.[f] ?? "";
  showVariant(p && !p.texts.any ? (VARIANTS.find((v) => p.texts[v]) ?? "any") : "any");
  input("post-card-on").checked = !!p?.card;
  input("post-card-lat").value = p?.card?.lat != null ? String(p.card.lat) : "";
  input("post-card-lon").value = p?.card?.lon != null ? String(p.card.lon) : "";
  input("post-card-width").value = String(p?.card?.width_m ?? 900);
  input("post-card-title").value = p?.card?.title ?? "";
  input("post-card-note").value = p?.card?.note ?? "";
  $("post-preview").replaceChildren();
  $("post-error").textContent = "";
  syncTrigger();
  $("post-form").hidden = false;
  input("post-name").focus();
}

function cardOf(): Card | null {
  if (!input("post-card-on").checked) return null;
  const c: Card = { width_m: Number(input("post-card-width").value) || 900, title: input("post-card-title").value.trim(), note: input("post-card-note").value.trim() };
  const lat = input("post-card-lat").value.trim();
  const lon = input("post-card-lon").value.trim();
  if (lat || lon) Object.assign(c, { lat: Number(lat.replace(",", ".")), lon: Number(lon.replace(",", ".")) });
  return c;
}

async function save() {
  keepShown();
  const tr = $<HTMLSelectElement>("post-trigger").value;
  const body = {
    id: editing?.id,
    title: input("post-name").value.trim(),
    trigger: tr,
    at: tr === "at" ? fromLocal(input("post-at").value) : null,
    chapter: tr.startsWith("chapter_") ? $<HTMLSelectElement>("post-chapter").value : null,
    offset_min: Math.round(Number(input("post-offset").value) || 0),
    texts: Object.fromEntries(Object.entries(texts).filter(([, v]) => v && (v.instagram || v.mastodon))),
    card: cardOf(),
    channels: ["instagram", "mastodon"].filter((c) => input(`post-ch-${c}`).checked),
  };
  const res = await api(`/api/ops/posts?${q()}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("post-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("post-form");
  toast(t("po.saved"));
  $("post-form").hidden = true;
  await loadPosts(region, primary, chapters);
}

/** A picture through the token: fetched as a blob. */
async function picture(path: string): Promise<string | null> {
  const res = await api(path).catch(() => null);
  if (!res?.ok) return null;
  return URL.createObjectURL(await res.blob());
}

async function preview() {
  const c = cardOf() ?? { width_m: 900, title: "", note: "" };
  const p = new URLSearchParams({ region, width_m: String(c.width_m), title: c.title, note: c.note });
  if (c.lat != null && c.lon != null) p.set("lat", String(c.lat)), p.set("lon", String(c.lon));
  const tr = $<HTMLSelectElement>("post-trigger").value;
  if (tr === "chapter_end") {
    const k = $<HTMLSelectElement>("post-chapter").value;
    const ch = chapters.find((x) => x.key === k);
    if (ch && ch.ends < Date.now() / 1000) p.set("chapter", k);
  }
  const host = $("post-preview");
  host.replaceChildren(el("small", { textContent: t("po.preview.wait") }));
  const url = await picture(`/api/ops/card.jpg?${p}`);
  host.replaceChildren(url ? el("img", { src: url, className: "post-pic", alt: "" }) : el("small", { textContent: t("po.preview.fail") }));
}

const OUTCOME: Record<string, EKey> = { fire: "po.v.fire", water: "po.v.water", tie: "po.v.tie" };

function whenOf(p: Post): string {
  const off = p.offset_min ? ` ${p.offset_min > 0 ? "+" : "−"}${Math.abs(p.offset_min)} min` : "";
  switch (p.trigger) {
    case "at":
      return fmtDate(p.at!);
    case "manual":
      return t("po.tr.manual");
    default:
      return `${t(p.trigger === "chapter_start" ? "po.tr.start" : "po.tr.end")}: ${chapterName(p.chapter)}${off}`;
  }
}

function copyButton(text: () => string) {
  return el("button", {
    type: "button",
    textContent: t("po.copy"),
    onclick: async () => {
      await navigator.clipboard.writeText(text()).then(
        () => toast(t("po.copied")),
        () => toast(t("po.copyfail"), "error"),
      );
    },
  });
}

async function decide(r: Run, state: Run["state"], edited?: Texts) {
  const res = await api(`/api/ops/posts/run?${q()}&id=${r.id}`, { method: "POST", body: JSON.stringify({ state, texts: edited }) }).catch(() => null);
  if (!res?.ok) toast(await errorOf(res, t("ui.savefail")), "error");
  else toast(t(state === "sent" ? "po.marked" : state === "discarded" ? "po.discarded" : "po.reopened"));
  await loadPosts(region, primary, chapters);
}

async function send(r: Run, ch: string, edited: Texts | undefined, btn: HTMLButtonElement) {
  if (!(await ask(t("po.send.ask", { ch: t(LABEL[ch]) }), t("po.send")))) return;
  btn.disabled = true;
  const res = await api(`/api/ops/posts/send?${q()}&id=${r.id}&channel=${ch}`, { method: "POST", body: JSON.stringify({ texts: edited }) }).catch(() => null);
  if (res?.ok) toast(t("po.sent"));
  else toast(t("po.send.err", { err: await errorOf(res, t("ui.savefail")) }), "error");
  await loadPosts(region, primary, chapters);
}

function drawRun(r: Run): HTMLElement {
  const p = posts.find((x) => x.id === r.post);
  const box = el("div", { className: `post-run ${r.state}` });
  const head = el("div", { className: "map-head" });
  const h = el("h3", { textContent: p?.title ?? `#${r.post}` });
  const meta = el("small", { textContent: fmtDate(r.created) });
  if (r.outcome) meta.textContent += ` · ${t(OUTCOME[r.outcome])}`;
  if (r.numbers) meta.textContent += ` · 🔥 ${r.numbers.fire} · 💧 ${r.numbers.water}`;
  if (r.state !== "draft") meta.textContent += ` · ${t(r.state === "sent" ? "po.st.sent" : "po.st.discarded")}${r.decided_by ? ` (${r.decided_by})` : ""}`;
  head.append(h, meta);
  box.append(head);
  if (r.note) box.append(el("p", { className: "notice", textContent: r.note }));
  const body = el("div", { className: "post-run-body" });
  if (r.image) {
    const fig = el("div", { className: "post-run-pic" });
    const known = pictures.get(r.id);
    const img = el("img", { className: "post-pic", alt: "" });
    const dl = el("a", { textContent: t("po.download"), className: "ed-link", download: `post-${r.id}.jpg` });
    const set = (u: string) => ((img.src = u), (dl.href = u));
    if (known) set(known);
    else
      void picture(`/api/ops/posts/run.jpg?${q()}&id=${r.id}`).then((u) => {
        if (u) pictures.set(r.id, u), set(u);
      });
    fig.append(img, dl);
    body.append(fig);
  }
  const fields = el("div", { className: "post-run-texts" });
  const areas = new Map<string, HTMLTextAreaElement>();
  const edited = () => Object.fromEntries([...areas].map(([k, a]) => [k, a.value.trim()])) as Texts;
  const changed = () => [...areas].some(([k, a]) => a.value.trim() !== (r.texts[k as keyof Texts] ?? "").trim());
  for (const f of FIELDS) {
    if (r.texts[f] == null) continue;
    const a = document.createElement("textarea");
    Object.assign(a, { value: r.texts[f]!, maxLength: MAX[f], readOnly: r.state !== "draft" });
    areas.set(f, a);
    const lab = el("label");
    const row = el("span", { className: "post-run-label" });
    const tools = el("span");
    tools.append(copyButton(() => a.value));
    const done = r.results?.[f];
    if (done?.url) tools.append(el("a", { className: "ed-link", href: done.url, target: "_blank", rel: "noopener", textContent: t("po.sent.view") }));
    else if (r.state === "draft" && ready.includes(f)) {
      const b = el("button", { type: "button", textContent: t("po.send") });
      b.onclick = () => void send(r, f, changed() ? edited() : undefined, b);
      tools.append(b);
    }
    row.append(el("b", { textContent: t(LABEL[f]) }), tools);
    lab.append(row, a);
    if (done?.error && !done.url) lab.append(el("small", { className: "error", textContent: t("po.send.err", { err: done.error }) }));
    fields.append(lab);
  }
  body.append(fields);
  box.append(body);
  const acts = el("div", { className: "ed-row" });
  if (r.state === "draft") {
    acts.append(
      el("button", { type: "button", className: "primary", textContent: t("po.mark"), onclick: () => void decide(r, "sent", changed() ? edited() : undefined) }),
      el("button", {
        type: "button",
        textContent: t("po.discard"),
        onclick: async () => {
          if (await ask(t("po.discard.ask"), t("po.discard"), true)) await decide(r, "discarded");
        },
      }),
    );
  } else {
    acts.append(el("button", { type: "button", textContent: t("po.reopen"), onclick: () => void decide(r, "draft") }));
  }
  box.append(acts);
  return box;
}

function drawRuns() {
  const host = $("post-runs");
  const open = runs.filter((r) => r.state === "draft");
  const done = runs.filter((r) => r.state !== "draft").slice(0, 10);
  host.replaceChildren();
  if (!open.length) host.append(el("p", { className: "empty", textContent: t("po.runs.none") }));
  for (const r of open) host.append(drawRun(r));
  if (done.length) {
    const d = el("details", { className: "post-done" });
    d.append(el("summary", { textContent: t("po.runs.done", { n: done.length }) }), ...done.map(drawRun));
    host.append(d);
  }
}

function drawList() {
  const host = $("posts-list");
  if (!posts.length) {
    host.className = "empty";
    host.textContent = t("po.none");
    return;
  }
  host.className = "";
  const tbl = el("table", { className: "ed-table" });
  const hr = tbl.createTHead().insertRow();
  for (const h of [t("po.th.post"), t("po.th.when"), t("ch.th.status"), ""]) hr.append(el("th", { textContent: h }));
  for (const p of posts) {
    const tr = tbl.insertRow();
    const name = tr.insertCell();
    name.append(el("b", { textContent: p.title }), el("br"), el("small", { textContent: [...p.channels.map((c) => t(LABEL[c] ?? "po.f.mastodon")), ...(p.card ? [t("po.f.card")] : [])].join(" · ") }));
    tr.insertCell().textContent = whenOf(p);
    const mine = runs.filter((r) => r.post === p.id);
    const st = tr.insertCell();
    const last = mine[0];
    st.append(el("span", { className: `st ${last?.state === "draft" ? "live" : ""}`, textContent: last ? t(last.state === "draft" ? "po.st.draft" : last.state === "sent" ? "po.st.sent" : "po.st.discarded") : t("st.planned") }));
    const td = tr.insertCell().appendChild(el("div", { className: "post-acts" }));
    const b = (label: string, fn: () => void) => td.append(el("button", { type: "button", textContent: label, onclick: fn }));
    b(t("ui.edit"), async () => {
      if (await mayLeave("post-form")) open(p);
    });
    b(t("po.draftnow"), async () => {
      const res = await api(`/api/ops/posts/draft?${q()}&id=${p.id}`, { method: "POST" }).catch(() => null);
      if (!res?.ok) toast(await errorOf(res, t("ui.savefail")), "error");
      else toast(t("po.drafted"));
      await loadPosts(region, primary, chapters);
    });
    b(t("ui.delete"), async () => {
      if (!(await ask(t("po.delete.ask", { name: p.title }), t("ui.delete"), true))) return;
      const res = await api(`/api/ops/posts?${q()}&id=${p.id}`, { method: "DELETE" });
      if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
      else toast(t("po.deleted"));
      await loadPosts(region, primary, chapters);
    });
  }
  host.replaceChildren(tbl);
}

export function initPosts(apiFn: Api) {
  api = apiFn;
  // The placeholders shown as such (t() would fill them).
  $("post-variants-hint").innerHTML = t("po.f.variants.hint", { fire: "{fire}", water: "{water}", chapter: "{chapter}" });
  $("post-new").onclick = async () => {
    if (await mayLeave("post-form")) open(null);
  };
  watchForm("post-form");
  $("post-cancel").onclick = async () => {
    if (!(await mayLeave("post-form"))) return;
    $("post-form").hidden = true;
  };
  $("post-save").onclick = () => void save();
  $("post-preview-btn").onclick = () => void preview();
  $("post-trigger").onchange = syncTrigger;
  input("post-card-on").onchange = syncTrigger;
  for (const f of FIELDS) area(`post-${f}`).oninput = counts;
  const vs = $("post-variants");
  vs.replaceChildren(
    ...VARIANTS.map((v) => el("button", { type: "button", textContent: t(`po.v.${v}` as EKey), onclick: () => showVariant(v) })),
  );
  [...vs.children].forEach((b, i) => ((b as HTMLElement).dataset.v = VARIANTS[i]));
}

export async function loadPosts(slug: string, lang: string, chs: ChapterRef[]) {
  if (slug !== region) {
    for (const u of pictures.values()) URL.revokeObjectURL(u);
    pictures.clear();
  }
  region = slug;
  primary = lang;
  chapters = [...chs].sort((a, b) => b.starts - a.starts);
  const res = await api(`/api/ops/posts?${q()}`).catch(() => null);
  $("posts-card").hidden = !res?.ok;
  if (!res?.ok) return;
  ({ posts, runs, ready } = (await res.json()) as { posts: Post[]; runs: Run[]; ready: string[] });
  drawRuns();
  drawList();
  const n = runs.filter((r) => r.state === "draft").length;
  const badge = document.querySelector('#tabs button[data-tab="beitraege"] .badge');
  if (badge) badge.textContent = n ? String(n) : "";
}
