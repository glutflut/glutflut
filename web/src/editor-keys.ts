// Redaktion: keys of the city (server: editor_keys.rs). Links that open one
// of the city's hotspot sites to players in other cities: at their own
// permanent source there, they claim around it, either side. Made in batches
// with a label; the links are shown once (only their hashes are kept).
import qrcode from "qrcode-generator";
import { ask, errorOf, fmtDate, toast } from "./editor-ui";
import { t } from "./editor-i18n";

type Site = { id: number; name: string };
type Batch = { batch: string; label: string | null; site: string | null; links: number; uses: number; made: number; expires: number };
type Api = (path: string, init?: RequestInit) => Promise<Response>;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);

let api: Api;
let region = "";
let sites: Site[] = [];
let batches: Batch[] = [];

/** The app's link for a key (gift.ts reads `.city`). */
const link = (key: string) => `${location.origin}/#gift=${region}.${key}.city`;

async function open() {
  // The hotspot sites as they are now (one may have just been added under "Orte").
  await loadKeys(region);
  const sel = $<HTMLSelectElement>("keys-site");
  sel.replaceChildren(...sites.map((s) => Object.assign(document.createElement("option"), { value: String(s.id), textContent: s.name })));
  $("keys-error").textContent = sites.length ? "" : t("ky.nosites");
  $<HTMLButtonElement>("keys-make").disabled = !sites.length;
  $("keys-out").hidden = true;
  $("keys-form").hidden = false;
  input("keys-label").focus();
}

async function make() {
  const body = {
    site: Number($<HTMLSelectElement>("keys-site").value),
    label: input("keys-label").value.trim(),
    count: Number(input("keys-count").value),
    uses: Number(input("keys-uses").value),
    days: Number(input("keys-days").value),
  };
  const res = await api(`/api/ops/keys?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("keys-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  const { keys } = (await res.json()) as { keys: string[] };
  $("keys-form").hidden = true;
  const links = keys.map(link);
  $<HTMLTextAreaElement>("keys-links").value = links.join("\n");
  // One QR for a single link (a poster); many links go out as a list.
  const qr = $("keys-qr");
  qr.replaceChildren();
  if (links.length === 1) {
    const q = qrcode(0, "M");
    q.addData(links[0]);
    q.make();
    qr.innerHTML = q.createSvgTag({ cellSize: 5, margin: 2, scalable: true });
  }
  $("keys-out").hidden = false;
  toast(t("ky.made", { n: links.length }));
  await loadKeys(region);
}

function drawList() {
  const el = $("keys-list");
  if (!batches.length) {
    el.className = "empty";
    el.textContent = t("ky.none");
    return;
  }
  el.className = "";
  const now = Date.now() / 1000;
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  const hr = tbl.createTHead().insertRow();
  for (const h of [t("ky.th.label"), t("ky.f.site"), t("ky.th.left"), t("ch.th.status"), ""]) hr.append(Object.assign(document.createElement("th"), { textContent: h }));
  for (const b of batches) {
    const tr = tbl.insertRow();
    tr.insertCell().append(Object.assign(document.createElement("b"), { textContent: b.label ?? "" }), document.createElement("br"), Object.assign(document.createElement("small"), { textContent: fmtDate(b.made) }));
    tr.insertCell().textContent = b.site ?? "–";
    tr.insertCell().textContent = t("ky.left", { links: b.links, uses: b.uses });
    const live = now < b.expires;
    const st = tr.insertCell();
    st.append(Object.assign(document.createElement("span"), { className: `st ${live ? "live" : ""}`, textContent: t(live ? "st.live" : "st.over") }), document.createElement("br"), Object.assign(document.createElement("small"), { textContent: t("ch.until", { date: fmtDate(b.expires) }) }));
    const td = tr.insertCell();
    td.append(
      Object.assign(document.createElement("button"), {
        textContent: t("ky.withdraw"),
        onclick: async () => {
          if (!(await ask(t("ky.withdraw.ask", { name: b.label ?? "" }), t("ky.withdraw"), true))) return;
          const res = await api(`/api/ops/keys?region=${encodeURIComponent(region)}&batch=${b.batch}`, { method: "DELETE" });
          if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
          else toast(t("ky.withdrawn"));
          await loadKeys(region);
        },
      }),
    );
  }
  el.replaceChildren(tbl);
}

export function initKeys(apiFn: Api) {
  api = apiFn;
  $("keys-new").onclick = () => void open();
  $("keys-cancel").onclick = () => ($("keys-form").hidden = true);
  $("keys-make").onclick = () => void make();
  $("keys-done").onclick = () => ($("keys-out").hidden = true);
  $("keys-copy").onclick = () => {
    navigator.clipboard?.writeText($<HTMLTextAreaElement>("keys-links").value).then(() => toast(t("ky.copied")), () => {});
  };
}

export async function loadKeys(slug: string) {
  region = slug;
  const res = await api(`/api/ops/keys?region=${encodeURIComponent(slug)}`).catch(() => null);
  $("keys-card").hidden = !res?.ok;
  if (!res?.ok) return;
  ({ sites, batches } = (await res.json()) as { sites: Site[]; batches: Batch[] });
  drawList();
}
