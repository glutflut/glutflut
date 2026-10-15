// Redaktion: the city's campaign phrases (server: editor_mottos.rs). Short
// texts, so languages are rows (the city's own always, the rest folded away), not tabs.
// Editors propose, admins approve.
import { ask, clean, errorOf, fmtDate, LANGS, langName, mayLeave, toast, watchForm } from "./editor-ui";
import { t } from "./editor-i18n";

type Motto = {
  num: number;
  campaign: string;
  side: "fire" | "water" | "both";
  names: Record<string, string>;
  status: "proposed" | "approved";
  by: string | null;
  approved_by: string | null;
};
type Campaign = { campaign: string; starts: number; ends: number };
type Api = (path: string, init?: RequestInit) => Promise<Response>;

const SIDE_KEY = { fire: "mo.side.fire", water: "mo.side.water", both: "mo.side.both" } as const;
const MAX = 40;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);

let api: Api;
let region = "";
let admin = false;
/** The city's language (required); the others fold away. */
let primary = "de";
const other = () => LANGS.filter((l) => l !== primary);
let mottos: Motto[] = [];
let campaigns: Campaign[] = [];
let editing: Motto | null = null;

const date = (s: number) => fmtDate(s, false);

function drawLangs(names: Record<string, string>) {
  const host = $("motto-langs");
  host.replaceChildren(
    ...other().flatMap((l) => {
      const label = Object.assign(document.createElement("span"), { textContent: langName(l) });
      const i = Object.assign(document.createElement("input"), { id: `motto-${l}`, maxLength: MAX, value: names[l] ?? "" });
      i.oninput = count;
      return [label, i];
    }),
  );
  count();
}

function count() {
  const n = other().filter((l) => input(`motto-${l}`)?.value.trim()).length;
  $("motto-langs-count").textContent = n ? t("mo.count", { n, m: other().length }) : "";
}

function open(m: Motto | null) {
  clean("motto-form");
  editing = m;
  $("motto-title").textContent = m ? t("mo.edit", { status: t(m.status === "approved" ? "mo.st.approved" : "mo.st.proposed") }) : t("mo.new");
  const sel = $<HTMLSelectElement>("motto-campaign");
  sel.replaceChildren(...campaigns.map((c) => new Option(`${c.campaign} (${date(c.starts)} – ${date(c.ends)})`, c.campaign)));
  if (m && !campaigns.some((c) => c.campaign === m.campaign)) sel.append(new Option(m.campaign, m.campaign));
  sel.value = m?.campaign ?? campaigns[0]?.campaign ?? "";
  $<HTMLSelectElement>("motto-side").value = m?.side ?? "both";
  $("motto-main-label").textContent = t("mo.citylang", { lang: langName(primary) });
  input("motto-main").value = m?.names[primary] ?? "";
  drawLangs(m?.names ?? {});
  ($("motto-langs").parentElement as HTMLDetailsElement).open = other().some((l) => m?.names[l]);
  $("motto-approve").hidden = !admin;
  $("motto-error").textContent = "";
  $("motto-form").hidden = false;
  input("motto-main").focus();
}

async function save(approve: boolean) {
  const names: Record<string, string> = { [primary]: input("motto-main").value.trim() };
  for (const l of other()) names[l] = input(`motto-${l}`).value.trim();
  const body = { num: editing?.num, campaign: $<HTMLSelectElement>("motto-campaign").value, side: $<HTMLSelectElement>("motto-side").value, names, approve };
  const res = await api(`/api/ops/mottos?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("motto-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("motto-form");
  toast(t(approve ? "mo.saved.approved" : "mo.saved.proposed"));
  $("motto-form").hidden = true;
  await loadMottos(region, admin, primary);
}

async function setStatus(m: Motto) {
  const res = await api(`/api/ops/mottos?region=${encodeURIComponent(region)}`, {
    method: "POST",
    body: JSON.stringify({ num: m.num, campaign: m.campaign, side: m.side, names: m.names, approve: true }),
  });
  if (!res.ok) toast(await errorOf(res, t("ui.approvefail")), "error");
  else toast(t("mo.approved"));
  await loadMottos(region, admin, primary);
}

function drawList() {
  const el = $("mottos-list");
  if (!mottos.length) {
    el.className = "empty";
    el.textContent = t(campaigns.length ? "mo.none" : "mo.first");
    return;
  }
  el.className = "";
  const now = Date.now() / 1000;
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  const hr = tbl.createTHead().insertRow();
  for (const h of [t("mo.th.text"), t("mo.th.campaign"), t("mo.th.side"), t("mo.th.status"), ""]) hr.append(Object.assign(document.createElement("th"), { textContent: h }));
  for (const m of mottos) {
    const tr = tbl.insertRow();
    const text = tr.insertCell();
    text.className = "motto-text";
    text.append(m.names[primary] ?? Object.values(m.names)[0]);
    const more = other().filter((l) => m.names[l]);
    if (more.length) text.append(document.createElement("br"), Object.assign(document.createElement("small"), { textContent: more.map((l) => `${langName(l)}: ${m.names[l]}`).join(" · ") }));
    const c = campaigns.find((x) => x.campaign === m.campaign);
    const running = c && c.starts <= now && now < c.ends;
    tr.insertCell().append(m.campaign, ...(running ? [document.createElement("br"), Object.assign(document.createElement("small"), { textContent: t("st.live") })] : []));
    tr.insertCell().textContent = t(SIDE_KEY[m.side]);
    const st = tr.insertCell();
    st.append(Object.assign(document.createElement("span"), { className: `st ${m.status === "approved" && running ? "live" : ""}`, textContent: t(m.status === "approved" ? "mo.st.approved" : "mo.st.waiting") }));
    const who = m.status === "approved" ? m.approved_by : m.by;
    if (who) st.append(document.createElement("br"), Object.assign(document.createElement("small"), { textContent: who }));
    const td = tr.insertCell();
    const b = (label: string, fn: () => void) => td.append(Object.assign(document.createElement("button"), { textContent: label, onclick: fn }));
    b(t("ui.edit"), () => open(m));
    if (admin && m.status !== "approved") b(t("ui.approve"), () => void setStatus(m));
    b(t("ui.delete"), async () => {
      if (!(await ask(t("mo.delete.ask", { name: m.names[primary] ?? Object.values(m.names)[0] }), t("ui.delete"), true))) return;
      const res = await api(`/api/ops/mottos?region=${encodeURIComponent(region)}&num=${m.num}`, { method: "DELETE" });
      if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
      else toast(t("mo.deleted"));
      await loadMottos(region, admin, primary);
    });
  }
  el.replaceChildren(tbl);
}

export function initMottos(apiFn: Api) {
  api = apiFn;
  $("motto-new").onclick = () => open(null);
  watchForm("motto-form");
  $("motto-cancel").onclick = async () => {
    if (!(await mayLeave("motto-form"))) return;
    $("motto-form").hidden = true;
  };
  $("motto-save").onclick = () => void save(false);
  $("motto-approve").onclick = () => void save(true);
}

export async function loadMottos(slug: string, isAdmin: boolean, lang: string) {
  region = slug;
  admin = isAdmin;
  primary = lang;
  const res = await api(`/api/ops/mottos?region=${encodeURIComponent(slug)}`).catch(() => null);
  $("mottos-card").hidden = !res?.ok;
  if (!res?.ok) return;
  ({ mottos, campaigns } = (await res.json()) as { mottos: Motto[]; campaigns: Campaign[] });
  $("motto-new").hidden = !campaigns.length;
  drawList();
}
