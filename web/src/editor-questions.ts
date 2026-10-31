// Redaktion: the city's questions (server: editor_questions.rs). A question
// with two to four answers, at one of the city's places (with a radius) or
// anywhere, for a period; the app asks it in the "Wie ist es hier?" card.
// Results show once enough people answered. Languages as rows like the
// news flashes: the city's own always, the rest folded away.
import { ask, clean, errorOf, fmtDate, fromLocal, LANGS, langName, mayLeave, toast, toLocal, watchForm } from "./editor-ui";
import { t } from "./editor-i18n";

type Texts = Record<string, string>;
type Question = {
  id: number;
  names: Texts;
  answers: Texts[];
  site: number | null;
  radius_m: number;
  starts: number;
  ends: number;
  by: string | null;
  persons: number | null;
  results: number[] | null;
};
type Api = (path: string, init?: RequestInit) => Promise<Response>;

const Q_MAX = 120;
const A_MAX = 40;
const ANSWERS = 4;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);

let api: Api;
let region = "";
let primary = "de";
const other = () => LANGS.filter((l) => l !== primary);
let questions: Question[] = [];
let sites: { id: number; name: string }[] = [];
let k = 5;
let editing: Question | null = null;

/** One row per language: the question and up to four answers. */
function langRow(l: string, q: Question | null): HTMLElement[] {
  const label = Object.assign(document.createElement("span"), { textContent: langName(l) });
  const box = document.createElement("div");
  box.className = "q-row";
  box.append(Object.assign(document.createElement("input"), { id: `q-${l}`, maxLength: Q_MAX, value: q?.names[l] ?? "", placeholder: t("qu.f.question") }));
  for (let i = 0; i < ANSWERS; i++) {
    box.append(Object.assign(document.createElement("input"), { id: `q-${l}-${i}`, maxLength: A_MAX, value: q?.answers[i]?.[l] ?? "", placeholder: t("qu.f.answer", { n: i + 1 }) }));
  }
  return [label, box];
}

function open(q: Question | null) {
  clean("question-form");
  editing = q;
  $("question-title").textContent = q ? t("qu.edit") : t("qu.new");
  $("question-main").replaceChildren(...langRow(primary, q));
  $("question-langs").replaceChildren(...other().flatMap((l) => langRow(l, q)));
  ($("question-langs").parentElement as HTMLDetailsElement).open = other().some((l) => q?.names[l]);
  const sel = $<HTMLSelectElement>("question-site");
  sel.replaceChildren(new Option(t("qu.f.anywhere"), ""), ...sites.map((s) => new Option(s.name, String(s.id))));
  sel.value = q?.site ? String(q.site) : "";
  input("question-radius").value = String(q?.radius_m ?? 250);
  // Answers that people gave can't change any more.
  const started = !!q && q.starts <= Date.now() / 1000;
  for (const l of LANGS) for (let i = 0; i < ANSWERS; i++) if (input(`q-${l}-${i}`)) input(`q-${l}-${i}`).disabled = started;
  const now = Math.floor(Date.now() / 60_000) * 60;
  input("question-starts").value = toLocal(q?.starts ?? now);
  input("question-ends").value = toLocal(q?.ends ?? now + 14 * 86_400);
  $("question-error").textContent = "";
  $("question-form").hidden = false;
  input(`q-${primary}`).focus();
}

async function save() {
  const names: Texts = {};
  const answers: Texts[] = Array.from({ length: ANSWERS }, () => ({}));
  for (const l of LANGS) {
    const q = input(`q-${l}`)?.value.trim();
    if (q) names[l] = q;
    for (let i = 0; i < ANSWERS; i++) {
      const a = input(`q-${l}-${i}`)?.value.trim();
      if (a) answers[i][l] = a;
    }
  }
  const site = $<HTMLSelectElement>("question-site").value;
  const body = {
    id: editing?.id,
    names,
    answers: answers.filter((a) => Object.keys(a).length),
    site: site ? Number(site) : null,
    radius_m: Number(input("question-radius").value) || 250,
    starts: fromLocal(input("question-starts").value),
    ends: fromLocal(input("question-ends").value),
  };
  const res = await api(`/api/ops/questions?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("question-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("question-form");
  toast(t("qu.saved"));
  $("question-form").hidden = true;
  await loadQuestions(region, primary);
}

/** The answers with their shares as bars, or why there is nothing yet. */
function results(q: Question): HTMLElement {
  const box = document.createElement("div");
  box.className = "q-results";
  if (!q.results || q.persons === null) {
    box.append(Object.assign(document.createElement("small"), { textContent: t("qu.few", { k }) }));
    return box;
  }
  const total = q.results.reduce((a, b) => a + b, 0) || 1;
  q.answers.forEach((a, i) => {
    const pct = Math.round(((q.results?.[i] ?? 0) * 100) / total);
    const row = document.createElement("div");
    row.className = "q-bar";
    const bar = document.createElement("i");
    bar.style.width = `${pct}%`;
    row.append(Object.assign(document.createElement("span"), { textContent: `${a[primary] ?? Object.values(a)[0] ?? ""} · ${pct} %` }), bar);
    box.append(row);
  });
  box.append(Object.assign(document.createElement("small"), { textContent: t("qu.persons", { n: q.persons }) }));
  return box;
}

function drawList() {
  const el = $("questions-list");
  if (!questions.length) {
    el.className = "empty";
    el.textContent = t("qu.none");
    return;
  }
  el.className = "";
  const now = Date.now() / 1000;
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  const hr = tbl.createTHead().insertRow();
  for (const h of [t("qu.th.question"), t("qu.th.results"), t("ch.th.period"), t("ch.th.status"), ""]) hr.append(Object.assign(document.createElement("th"), { textContent: h }));
  for (const q of questions) {
    const tr = tbl.insertRow();
    const text = tr.insertCell();
    const title = q.names[primary] ?? Object.values(q.names)[0] ?? "";
    text.append(Object.assign(document.createElement("b"), { textContent: title }));
    const where = q.site ? `${sites.find((s) => s.id === q.site)?.name ?? "?"} (${q.radius_m} m)` : t("qu.f.anywhere");
    text.append(document.createElement("br"), Object.assign(document.createElement("small"), { textContent: where }));
    tr.insertCell().append(results(q));
    const when = tr.insertCell();
    when.append(fmtDate(q.starts), document.createElement("br"), Object.assign(document.createElement("small"), { textContent: t("ch.until", { date: fmtDate(q.ends) }) }));
    const st = tr.insertCell();
    const live = q.starts <= now && now < q.ends;
    st.append(Object.assign(document.createElement("span"), { className: `st ${live ? "live" : ""}`, textContent: t(live ? "st.live" : now < q.starts ? "st.planned" : "st.over") }));
    if (q.by) st.append(document.createElement("br"), Object.assign(document.createElement("small"), { textContent: q.by }));
    const td = tr.insertCell();
    const b = (label: string, fn: () => void) => td.append(Object.assign(document.createElement("button"), { textContent: label, onclick: fn }));
    b(t("ui.edit"), async () => {
      if (await mayLeave("question-form")) open(q);
    });
    b(t("ui.delete"), async () => {
      if (!(await ask(t("qu.delete.ask", { name: title }), t("ui.delete"), true))) return;
      const res = await api(`/api/ops/questions?region=${encodeURIComponent(region)}&id=${q.id}`, { method: "DELETE" });
      if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
      else toast(t("qu.deleted"));
      await loadQuestions(region, primary);
    });
  }
  el.replaceChildren(tbl);
}

export function initQuestions(apiFn: Api) {
  api = apiFn;
  $("question-new").onclick = async () => {
    if (await mayLeave("question-form")) open(null);
  };
  watchForm("question-form");
  $("question-cancel").onclick = async () => {
    if (!(await mayLeave("question-form"))) return;
    $("question-form").hidden = true;
  };
  $("question-save").onclick = () => void save();
}

export async function loadQuestions(slug: string, lang: string) {
  region = slug;
  primary = lang;
  const res = await api(`/api/ops/questions?region=${encodeURIComponent(slug)}`).catch(() => null);
  $("questions-card").hidden = !res?.ok;
  if (!res?.ok) return;
  const j = (await res.json()) as { sites: typeof sites; questions: Question[]; k: number };
  ({ sites, questions, k } = j);
  drawList();
}
