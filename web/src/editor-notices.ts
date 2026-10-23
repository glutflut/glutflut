// Redaktion: news flashes (server: editor_notices.rs). A title, a short
// text, maybe a link, and a period; the app shows a running one as a card
// once per device and under "what's new". Languages as rows like the tag
// lines: the city's own always, the rest folded away.
import { ask, clean, errorOf, fmtDate, fromLocal, LANGS, langName, mayLeave, toast, toLocal, watchForm } from "./editor-ui";
import { t } from "./editor-i18n";

type Names = Record<string, { title: string; text: string }>;
type Notice = { id: number; names: Names; url: string; starts: number; ends: number; by: string | null };
type Api = (path: string, init?: RequestInit) => Promise<Response>;

const TITLE_MAX = 60;
const TEXT_MAX = 400;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);

let api: Api;
let region = "";
let primary = "de";
const other = () => LANGS.filter((l) => l !== primary);
let notices: Notice[] = [];
let editing: Notice | null = null;

function drawLangs(names: Names) {
  const host = $("notice-langs");
  host.replaceChildren(
    ...other().flatMap((l) => {
      const label = Object.assign(document.createElement("span"), { textContent: langName(l) });
      const ti = Object.assign(document.createElement("input"), { id: `notice-${l}-title`, maxLength: TITLE_MAX, value: names[l]?.title ?? "", placeholder: t("nt.f.title.ph") });
      const xi = Object.assign(document.createElement("textarea"), { id: `notice-${l}-text`, maxLength: TEXT_MAX, value: names[l]?.text ?? "" });
      ti.oninput = count;
      return [label, ti, document.createElement("span"), xi];
    }),
  );
  count();
}

function count() {
  const n = other().filter((l) => input(`notice-${l}-title`)?.value.trim()).length;
  $("notice-langs-count").textContent = n ? t("mo.count", { n, m: other().length }) : "";
}

function open(n: Notice | null) {
  clean("notice-form");
  editing = n;
  $("notice-title").textContent = n ? t("nt.edit") : t("nt.new");
  $("notice-main-label").textContent = t("mo.citylang", { lang: langName(primary) });
  input("notice-main").value = n?.names[primary]?.title ?? "";
  $<HTMLTextAreaElement>("notice-text").value = n?.names[primary]?.text ?? "";
  drawLangs(n?.names ?? {});
  ($("notice-langs").parentElement as HTMLDetailsElement).open = other().some((l) => n?.names[l]?.title);
  input("notice-url").value = n?.url ?? "";
  const now = Math.floor(Date.now() / 60_000) * 60;
  input("notice-starts").value = toLocal(n?.starts ?? now);
  input("notice-ends").value = toLocal(n?.ends ?? now + 2 * 86_400);
  $("notice-error").textContent = "";
  $("notice-form").hidden = false;
  input("notice-main").focus();
}

async function save() {
  const names: Names = { [primary]: { title: input("notice-main").value.trim(), text: $<HTMLTextAreaElement>("notice-text").value.trim() } };
  for (const l of other()) names[l] = { title: input(`notice-${l}-title`).value.trim(), text: $<HTMLTextAreaElement>(`notice-${l}-text`).value.trim() };
  const body = { id: editing?.id, names, url: input("notice-url").value.trim(), starts: fromLocal(input("notice-starts").value), ends: fromLocal(input("notice-ends").value) };
  const res = await api(`/api/ops/notices?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("notice-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("notice-form");
  toast(t("nt.saved"));
  $("notice-form").hidden = true;
  await loadNotices(region, primary);
}

function drawList() {
  const el = $("notices-list");
  if (!notices.length) {
    el.className = "empty";
    el.textContent = t("nt.none");
    return;
  }
  el.className = "";
  const now = Date.now() / 1000;
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  const hr = tbl.createTHead().insertRow();
  for (const h of [t("nt.th.notice"), t("ch.th.period"), t("ch.th.status"), ""]) hr.append(Object.assign(document.createElement("th"), { textContent: h }));
  for (const n of notices) {
    const tr = tbl.insertRow();
    const text = tr.insertCell();
    const x = n.names[primary] ?? Object.values(n.names)[0];
    text.append(Object.assign(document.createElement("b"), { textContent: x?.title ?? "" }));
    if (x?.text) text.append(document.createElement("br"), Object.assign(document.createElement("small"), { textContent: x.text }));
    if (n.url) text.append(document.createElement("br"), Object.assign(document.createElement("a"), { href: n.url, textContent: n.url, target: "_blank", rel: "noopener noreferrer", className: "ed-link" }));
    const when = tr.insertCell();
    when.append(fmtDate(n.starts), document.createElement("br"), Object.assign(document.createElement("small"), { textContent: t("ch.until", { date: fmtDate(n.ends) }) }));
    const st = tr.insertCell();
    const live = n.starts <= now && now < n.ends;
    st.append(Object.assign(document.createElement("span"), { className: `st ${live ? "live" : ""}`, textContent: t(live ? "st.live" : now < n.starts ? "st.planned" : "st.over") }));
    if (n.by) st.append(document.createElement("br"), Object.assign(document.createElement("small"), { textContent: n.by }));
    const td = tr.insertCell();
    const b = (label: string, fn: () => void) => td.append(Object.assign(document.createElement("button"), { textContent: label, onclick: fn }));
    b(t("ui.edit"), async () => {
      if (await mayLeave("notice-form")) open(n);
    });
    b(t("ui.delete"), async () => {
      if (!(await ask(t("nt.delete.ask", { name: x?.title ?? "" }), t("ui.delete"), true))) return;
      const res = await api(`/api/ops/notices?region=${encodeURIComponent(region)}&id=${n.id}`, { method: "DELETE" });
      if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
      else toast(t("nt.deleted"));
      await loadNotices(region, primary);
    });
  }
  el.replaceChildren(tbl);
}

export function initNotices(apiFn: Api) {
  api = apiFn;
  $("notice-new").onclick = async () => {
    if (await mayLeave("notice-form")) open(null);
  };
  watchForm("notice-form");
  $("notice-cancel").onclick = async () => {
    if (!(await mayLeave("notice-form"))) return;
    $("notice-form").hidden = true;
  };
  $("notice-save").onclick = () => void save();
}

export async function loadNotices(slug: string, lang: string) {
  region = slug;
  primary = lang;
  const res = await api(`/api/ops/notices?region=${encodeURIComponent(slug)}`).catch(() => null);
  $("notices-card").hidden = !res?.ok;
  if (!res?.ok) return;
  notices = (await res.json()) as Notice[];
  drawList();
}
