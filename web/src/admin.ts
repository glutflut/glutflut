// Admin page (admin.html): one place for the operator across regions and
// shards — regions (`/api/ops/regions`), feedback (`/api/feedback`, mark
// done), images and campaign phrases waiting for approval (`/api/ops/pending`),
// "play here" wishes (`/api/wishes`), OSM updates (`/api/ops/updates`),
// maintenance windows (`/api/ops/maintenance`) and indoor QR sheets (indoor-render.ts;
// other shards' on their own indoor.html). Needs admin for all regions: the ops token, or a login
// (opsauth.ts) in the group `<prefix>admin`. The other shards come from
// this server's `peers` (ELEMENTS_ADMIN_PEERS, server admin.rs): the page asks
// them itself with the same login (cookie) or token.
import { authHeaders, getAccess, type Access } from "./opsauth";
import { renderIndoor, type IndoorReply } from "./indoor-render";

type Region = {
  slug: string;
  name: string;
  active: boolean;
  listed: boolean;
  rules: string | null;
  cell: string;
  language: string;
  indoor: boolean;
  loaded: boolean;
  players_today: number;
  players_7d: number;
  feedback_30d: number;
};
type Feedback = { id?: number; region: string; day: string; mood: number; text: string | null; lang: string | null; version: string | null; done_at?: string | null; done_by?: string | null };
type PendMedia = {
  id: number;
  region: string;
  region_name: string;
  kind: "find" | "zone" | "challenge";
  target: string;
  target_name: string | null;
  url: string | null;
  width: number;
  height: number;
  author: string;
  licence_label: string | null;
  licence_url: string | null;
  source_url: string | null;
  alt: string | null;
  by: string;
  created: number;
  bytes: number;
  replaces: string | null;
};
type PendMotto = { region: string; region_name: string; language: string; num: number; campaign: string; side: "fire" | "water" | "both"; names: Record<string, string>; by: string | null };
type UpdRegion = { region: string; name: string; active: boolean; rhythm: string | null; auto: boolean; last_check: number | null; last_write: number | null };
type UpdSummary = { verdict: string; stop: boolean; junctions: number; gone: number; new: number; gone_pct: number; sources_lost: number; sources_pct: number; finds_hidden: number; finds_new: number; refs_lost: number; seconds: number };
type UpdJob = { id: number; region: string; kind: string; status: string; by: string; created: number; started: number | null; finished: number | null; summary: UpdSummary | null; error: string | null; approved_by: string | null; run_now: boolean };
type Maint = { id: number; starts: number; ends: number; note: string | null; by: string; cancelled: number | null; cancelled_by: string | null };
type Wish = { cell: string; wishes: number; last: string; center: [number, number] | null };
type Listed = { slug: string; name: string; listed?: boolean; outline?: { type: string; coordinates: number[][][] | number[][][][] } };
/** A shard's server: `base` "" for this one, else its admin site. */
type Shard = { shard: string; base: string; play: string; state: "ok" | "wait" | string };
type At = { shard: string; base: string };

const TOKEN_KEY = "elements.ops"; // shared with the dashboard and the editor
const TAB_KEY = "elements.admin.tab";
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
const get = (k: string) => {
  try {
    return localStorage.getItem(k) ?? "";
  } catch {
    return "";
  }
};
const put = (k: string, v: string) => {
  try {
    if (v) localStorage.setItem(k, v);
    else localStorage.removeItem(k);
  } catch {
    // storage blocked: this page only
  }
};

let token = get(TOKEN_KEY);
let shards: Shard[] = [];
let regions: (Region & At)[] = [];
let feedback: (Feedback & At)[] = [];
let pendMedia: (PendMedia & At)[] = [];
let pendMottos: (PendMotto & At)[] = [];
let maint: (Maint & At)[] = [];
let updRegions: (UpdRegion & At)[] = [];
let updJobs: (UpdJob & At)[] = [];
let logoutTo: Access | null = null;
const msg = (s: string) => ($("msg").textContent = s);

/** A request to a shard: this one same-origin, others with the login cookie
 * (a text body keeps a POST simple: no preflight through the login proxy). */
const api = (base: string, path: string, init: RequestInit = {}) =>
  fetch(base + path, { ...init, credentials: base ? "include" : "same-origin", headers: { ...(init.headers ?? {}), ...authHeaders(token) } });

async function load<T>(s: Shard, path: string): Promise<T | null> {
  const res = await api(s.base, path).catch(() => null);
  if (!res?.ok) {
    s.state = res ? `Fehler ${res.status}` : "nicht erreichbar";
    return null;
  }
  return (await res.json().catch(() => null)) as T | null;
}

// --- access ---------------------------------------------------------------

async function checkAccess(): Promise<boolean> {
  const { a } = await getAccess(token);
  const login = !token && a?.user;
  $("token-field").hidden = !!login;
  $("logout").hidden = login ? !a?.logout : !token;
  const ok = !!a && a.all && a.role === "admin";
  $("token-state").textContent = !a ? (token ? "Token unbekannt" : "") : `${login ? `${a.user} · ` : ""}${ok ? "Admin, alle Regionen" : "kein Admin für alle Regionen"}`;
  $("tabs").hidden = !ok;
  if (!ok) for (const s of document.querySelectorAll<HTMLElement>(".tab")) s.hidden = true;
  logoutTo = a;
  return ok;
}

// --- shards ---------------------------------------------------------------

function drawShards() {
  $("shards").innerHTML = shards
    .map((s) => {
      const cls = s.state === "ok" ? "on" : s.state === "wait" ? "" : "bad";
      const title = s.state === "ok" ? (s.base || location.origin) : s.state;
      return `<span class="shard ${cls}" title="${esc(title)}">${esc(s.shard)}${s.state === "ok" || s.state === "wait" ? "" : " ⚠"}</span>`;
    })
    .join("");
  const bad = shards.filter((s) => s.state !== "ok" && s.state !== "wait");
  // With a token the browser must ask first (preflight), which a login proxy
  // in front of the other shard refuses: there only the login works.
  const why = token ? "mit Token oft nicht erreichbar, mit Login anmelden" : "dort angemeldet?";
  msg(bad.map((s) => `Shard ${s.shard}: ${s.state}${s.base ? ` (${s.base}; ${why})` : ""}`).join(" · "));
}

/** Play addresses by shard (the game links), from the shard index. */
async function playUrls(): Promise<Map<string, string>> {
  const res = await fetch("/api/marble/shards.json").catch(() => null);
  const j = res?.ok ? ((await res.json().catch(() => null)) as { shards?: { shard: string; play: string }[] } | null) : null;
  return new Map((j?.shards ?? []).map((s) => [s.shard, s.play.replace(/\/$/, "")]));
}

// --- regions --------------------------------------------------------------

const tag = (text: string, cls = "") => `<span class="tag ${cls}">${esc(text)}</span>`;
const many = () => shards.length > 1;
const playOf = (r: At) => (r.base ? (shards.find((s) => s.shard === r.shard)?.play ?? r.base) : "");

function renderRegions() {
  const q = $<HTMLInputElement>("region-q").value.trim().toLowerCase();
  const onlyActive = $<HTMLInputElement>("region-active").checked;
  const rows = regions.filter((r) => (!onlyActive || r.active) && (!q || `${r.name} ${r.slug} ${r.rules ?? ""} ${r.shard}`.toLowerCase().includes(q)));
  const sum = (k: "players_today" | "players_7d") => rows.reduce((n, r) => n + r[k], 0);
  $("region-sum").textContent = `${rows.length} von ${regions.length} · heute ${sum("players_today")} Spielende, 7 Tage ${sum("players_7d")}`;
  $("regions").innerHTML =
    `<tr><th>Region</th>${many() ? "<th>Shard</th>" : ""}<th>Stand</th><th>Regelsatz</th><th class="num">heute</th><th class="num">7 Tage</th><th class="num">Rückm. 30 T</th><th>Öffnen</th><th></th></tr>` +
    rows
      .map((r) => {
        const s = encodeURIComponent(r.slug);
        const state = [r.active ? tag("aktiv", "ok") : tag("aus", "off"), r.listed ? "" : tag("ungelistet"), r.loaded ? tag("geladen") : "", r.cell !== "main" ? tag(`Zelle ${r.cell}`) : ""].join("");
        const links = [
          `<a href="${playOf(r)}/?region=${s}" target="_blank">Spiel</a>`,
          `<a href="${r.base}/dashboard.html?region=${s}" target="_blank">Dashboard</a>`,
          `<a href="${r.base}/editor.html?region=${s}" target="_blank">Redaktion</a>`,
          r.indoor ? `<a href="${r.base}/indoor.html?region=${s}" target="_blank">Aushänge</a>` : "",
        ].join("");
        const reload = r.active ? `<button data-reload="${esc(r.slug)}" data-shard="${esc(r.shard)}">Neu laden</button>` : "";
        return `<tr><td>${esc(r.name)}<div class="slug">${esc(r.slug)} · ${esc(r.language)}</div></td>${many() ? `<td>${esc(r.shard)}</td>` : ""}<td>${state}</td><td>${esc(r.rules ?? "Standard")}${r.indoor ? ` ${tag("drinnen", "in")}` : ""}</td><td class="num">${r.players_today}</td><td class="num">${r.players_7d}</td><td class="num">${r.feedback_30d || ""}</td><td class="links">${links}</td><td>${reload}</td></tr>`;
      })
      .join("");
}

$("regions").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-reload]");
  if (!b) return;
  const slug = b.dataset.reload!;
  const s = shards.find((x) => x.shard === b.dataset.shard);
  if (!s || !confirm(`${slug} neu laden? Wer gerade spielt, wird kurz getrennt.`)) return;
  b.disabled = true;
  const res = await api(s.base, `/api/ops/reload?region=${encodeURIComponent(slug)}`, { method: "POST" }).catch(() => null);
  b.textContent = res?.ok ? "geladen ✓" : `Fehler ${res?.status ?? "Netz"}`;
});

// --- feedback -------------------------------------------------------------

const FACE: Record<number, string> = { 1: "☹️", 2: "😐", 3: "😊" };
const fbKey = (f: Feedback & At) => `${f.shard}:${f.id}`;

function renderFeedback() {
  const reg = $<HTMLSelectElement>("fb-region").value;
  const withText = $<HTMLInputElement>("fb-text").checked;
  const onlyOpen = $<HTMLInputElement>("fb-open").checked;
  const names = new Map(regions.map((r) => [r.slug, r.name]));
  const rows = feedback.filter((f) => (!reg || f.region === reg) && (!withText || f.text?.trim()) && (!onlyOpen || !f.done_at));
  const open = feedback.filter((f) => !f.done_at).length;
  $("fb-sum").textContent = `${rows.length} von ${feedback.length} (je Shard die letzten 500, ${open} offen)`;
  $("feedback").innerHTML =
    `<tr><th>Tag</th><th></th><th>Region</th><th>Text</th><th>Sprache</th><th>Version</th><th></th></tr>` +
    rows
      .map((f) => {
        const k = esc(fbKey(f));
        const done = f.done_at ? `<span class="slug" title="${esc(f.done_by ?? "")}">erledigt ${esc(f.done_at.slice(0, 10))}</span> <button data-done="${k}" data-to="0">wieder offen</button>` : `<button data-done="${k}" data-to="1">erledigt</button>`;
        const where = `${esc(names.get(f.region) ?? f.region)}${many() ? `<div class="slug">${esc(f.shard)}</div>` : ""}`;
        return `<tr${f.done_at ? ' class="done"' : ""}><td>${esc(f.day)}</td><td class="mood" title="${f.mood}">${FACE[f.mood] ?? f.mood}</td><td>${where}</td><td class="text">${esc(f.text ?? "")}</td><td>${esc(f.lang ?? "")}</td><td class="slug">${esc(f.version ?? "")}</td><td>${f.id === undefined ? "" : done}</td></tr>`;
      })
      .join("");
}

function feedbackRegions() {
  const sel = $<HTMLSelectElement>("fb-region");
  const keep = sel.value;
  const names = new Map(regions.map((r) => [r.slug, r.name]));
  const counts = new Map<string, number>();
  for (const f of feedback) counts.set(f.region, (counts.get(f.region) ?? 0) + 1);
  sel.innerHTML =
    `<option value="">alle Regionen</option>` +
    [...counts]
      .sort((a, b) => b[1] - a[1])
      .map(([s, n]) => `<option value="${esc(s)}">${esc(names.get(s) ?? s)} (${n})</option>`)
      .join("");
  sel.value = keep;
  badge();
}

/** Badge: open feedback with text. */
function badge() {
  const n = feedback.filter((f) => !f.done_at && f.text?.trim()).length;
  $("badge-fb").textContent = n ? String(n) : "";
}

$("feedback").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-done]");
  if (!b) return;
  const f = feedback.find((x) => fbKey(x) === b.dataset.done);
  if (!f) return;
  const done = b.dataset.to === "1";
  b.disabled = true;
  // A text body (text/plain): a simple request, also to another shard.
  const res = await api(f.base, `/api/ops/feedback/${f.id}`, { method: "POST", body: JSON.stringify({ done }) }).catch(() => null);
  if (!res?.ok) {
    b.disabled = false;
    return void msg(`Rückmeldung ${f.shard}/${f.id}: ${res ? `Fehler ${res.status}` : "kein Netz"}`);
  }
  f.done_at = done ? new Date().toISOString() : null;
  f.done_by = done ? (logoutTo?.user ?? "Token") : null;
  renderFeedback();
  badge();
});

// --- approvals ------------------------------------------------------------

const KIND: Record<PendMedia["kind"], string> = { find: "Fund", zone: "Gebiet", challenge: "Kapitel" };
const SIDE: Record<PendMotto["side"], string> = { fire: "Glut", water: "Flut", both: "beide" };
/** An image URL from another shard: relative ones (private bucket) are its own. */
const abs = (base: string, u: string) => (u.startsWith("/") ? base + u : u);
const day = (t: number) => new Date(t * 1000).toLocaleDateString("de-DE");
async function errText(res: Response | null) {
  if (!res) return "kein Netz";
  const t = (await res.text().catch(() => "")).trim().slice(0, 200);
  return `Fehler ${res.status}${t ? `: ${t}` : ""}`;
}

function renderPending() {
  const n = pendMedia.length + pendMottos.length;
  $("badge-pend").textContent = n ? String(n) : "";
  $("pend-media-sum").textContent = pendMedia.length ? `(${pendMedia.length})` : "";
  $("pend-mottos-sum").textContent = pendMottos.length ? `(${pendMottos.length})` : "";
  $("pend-media").innerHTML = pendMedia.length
    ? pendMedia
        .map((m) => {
          const k = esc(`${m.shard}:${m.id}`);
          const img = m.url ? `<img src="${esc(abs(m.base, m.url))}" alt="${esc(m.alt ?? "")}" loading="lazy" />` : "";
          const author = m.source_url ? `<a href="${esc(m.source_url)}" target="_blank" rel="noopener noreferrer">${esc(m.author)}</a>` : esc(m.author);
          const lic = m.licence_label ? (m.licence_url ? `, <a href="${esc(m.licence_url)}" target="_blank" rel="noopener noreferrer">${esc(m.licence_label)}</a>` : `, ${esc(m.licence_label)}`) : "";
          const was = m.replaces ? `<br><small>ersetzt</small> <img class="was" src="${esc(abs(m.base, m.replaces))}" alt="" loading="lazy" />` : "";
          const ed = `<a href="${m.base}/editor.html?region=${encodeURIComponent(m.region)}" target="_blank">Redaktion</a>`;
          return `<div class="card">${img}<div><b>${esc(KIND[m.kind] ?? m.kind)}: ${esc(m.target_name ?? m.target)}</b><br>${esc(m.region_name)}${many() ? ` · ${esc(m.shard)}` : ""}<br><small>Foto: ${author}${lic}</small><br><small>${m.width}×${m.height}, ${Math.round(m.bytes / 1000)} kB · von ${esc(m.by)} · ${day(m.created)}</small>${m.alt ? `<br><small>Alt: ${esc(m.alt)}</small>` : ""}${was}<div class="row"><button data-media="${k}" data-approve="1">Freigeben</button><button data-media="${k}" data-approve="0">Ablehnen</button>${ed}</div></div></div>`;
        })
        .join("")
    : `<p class="empty">Keine Bilder offen.</p>`;
  $("pend-mottos").innerHTML = pendMottos.length
    ? `<tr><th>Text</th><th>Region</th><th>Kampagne</th><th>Seite</th><th>von</th><th></th></tr>` +
      pendMottos
        .map((m) => {
          const k = esc(`${m.shard}:${m.region}:${m.num}`);
          const main = m.names[m.language] ?? Object.values(m.names)[0] ?? "";
          const more = Object.entries(m.names).filter(([l, v]) => l !== m.language && v);
          const ed = `<a href="${m.base}/editor.html?region=${encodeURIComponent(m.region)}" target="_blank">Redaktion</a>`;
          return `<tr><td class="text"><b>${esc(main)}</b>${more.length ? `<div class="slug">${more.map(([l, v]) => `${esc(l)}: ${esc(v)}`).join(" · ")}</div>` : ""}</td><td>${esc(m.region_name)}${many() ? `<div class="slug">${esc(m.shard)}</div>` : ""}</td><td>${esc(m.campaign)}</td><td>${esc(SIDE[m.side] ?? m.side)}</td><td>${esc(m.by ?? "")}</td><td class="links"><button data-motto="${k}">Freigeben</button> ${ed}</td></tr>`;
        })
        .join("")
    : `<tr><td class="empty">Keine Mottos offen.</td></tr>`;
}

$("pend-media").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-media]");
  if (!b) return;
  const m = pendMedia.find((x) => `${x.shard}:${x.id}` === b.dataset.media);
  if (!m) return;
  const approve = b.dataset.approve === "1";
  const name = `${KIND[m.kind] ?? m.kind} „${m.target_name ?? m.target}“ (${m.region_name})`;
  if (!confirm(approve ? `Bild für ${name} freigeben?${m.replaces ? " Das bisherige wird ersetzt." : ""}` : `Bild für ${name} ablehnen? Die Datei wird gelöscht.`)) return;
  b.disabled = true;
  // A text body: a simple request, also to another shard.
  const res = await api(m.base, `/api/ops/media/decide?region=${encodeURIComponent(m.region)}`, { method: "POST", body: JSON.stringify({ id: m.id, approve }) }).catch(() => null);
  if (!res?.ok) {
    b.disabled = false;
    return void msg(`Bild ${m.shard}/${m.id}: ${await errText(res)}`);
  }
  pendMedia = pendMedia.filter((x) => x !== m);
  msg("");
  renderPending();
});

$("pend-mottos").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-motto]");
  if (!b) return;
  const m = pendMottos.find((x) => `${x.shard}:${x.region}:${x.num}` === b.dataset.motto);
  if (!m) return;
  b.disabled = true;
  const body = { num: m.num, campaign: m.campaign, side: m.side, names: m.names, approve: true };
  const res = await api(m.base, `/api/ops/mottos?region=${encodeURIComponent(m.region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    b.disabled = false;
    return void msg(`Motto ${m.region}/${m.num}: ${await errText(res)}`);
  }
  pendMottos = pendMottos.filter((x) => x !== m);
  msg("");
  renderPending();
});

// --- wishes ---------------------------------------------------------------

/** Ray casting over the outer ring(s). */
function inside(pt: [number, number], outline: Listed["outline"]): boolean {
  if (!outline) return false;
  const polys = (outline.type === "MultiPolygon" ? outline.coordinates : [outline.coordinates]) as number[][][][];
  return polys.some((poly) => {
    const ring = poly[0];
    let c = false;
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      const [xi, yi] = ring[i];
      const [xj, yj] = ring[j];
      if (yi > pt[1] !== yj > pt[1] && pt[0] < ((xj - xi) * (pt[1] - yi)) / (yj - yi) + xi) c = !c;
    }
    return c;
  });
}

type WishRow = Wish & { region?: Listed };
let wishes: WishRow[] | null = null;

/** Every shard's wishes, the same cell summed; with the active region it lies in, if any. */
async function loadWishes() {
  const [lists, l] = await Promise.all([Promise.all(shards.map((s) => load<Wish[]>(s, "/api/wishes"))), fetch("/api/regions").then((r) => r.json()).catch(() => [])]);
  drawShards();
  const listed: Listed[] = Array.isArray(l) ? l : (l?.regions ?? []);
  const byCell = new Map<string, WishRow>();
  for (const w of lists.flatMap((x) => x ?? [])) {
    const o = byCell.get(w.cell);
    if (o) {
      o.wishes += w.wishes;
      if (w.last > o.last) o.last = w.last;
    } else byCell.set(w.cell, { ...w });
  }
  wishes = [...byCell.values()].sort((a, b) => b.wishes - a.wishes || b.last.localeCompare(a.last));
  for (const w of wishes) w.region = w.center ? listed.find((r) => inside(w.center!, r.outline)) : undefined;
}

function renderWishes() {
  if (!wishes) return;
  const f = $<HTMLSelectElement>("wish-filter").value;
  const rows = wishes.filter((w) => f === "all" || (f === "none" ? !w.region : f === "unlisted" ? w.region?.listed === false : !!w.region));
  const free = wishes.filter((w) => !w.region).length;
  $("wish-sum").textContent = `${rows.length} von ${wishes.length} Zellen · ${free} ohne Region · ${wishes.reduce((n, w) => n + w.wishes, 0)} Wünsche`;
  $("wishes").innerHTML =
    `<tr><th class="num">Wünsche</th><th>zuletzt</th><th>Ort</th><th>in Region</th><th>Zelle</th></tr>` +
    rows
      .map((c) => {
        const at = c.center;
        const map = at ? `<a href="https://www.openstreetmap.org/?mlat=${at[1].toFixed(5)}&mlon=${at[0].toFixed(5)}#map=11/${at[1].toFixed(4)}/${at[0].toFixed(4)}" target="_blank">${at[1].toFixed(3)}, ${at[0].toFixed(3)}</a>` : "";
        const reg = c.region ? `${esc(c.region.name)} ${c.region.listed === false ? tag("ungelistet") : tag("gelistet", "ok")}` : "";
        return `<tr><td class="num">${c.wishes}</td><td>${esc(c.last)}</td><td>${map}</td><td>${reg}</td><td class="slug">${esc(c.cell)}</td></tr>`;
      })
      .join("");
}

// --- QR codes -------------------------------------------------------------

const indoorKey = (r: At & { slug: string }) => `${r.shard}:${r.slug}`;
let codesShown = false;

function renderCodes() {
  const indoor = regions.filter((r) => r.indoor);
  const sel = $<HTMLSelectElement>("code-region");
  const keep = sel.value;
  sel.innerHTML = indoor.length
    ? indoor
        .map((r) => `<option value="${esc(indoorKey(r))}">${esc(r.name)} (${esc(r.slug)}${many() ? ` · ${esc(r.shard)}` : ""}${r.active ? "" : " · aus"}${r.listed ? "" : " · ungelistet"})</option>`)
        .join("")
    : `<option value="">Keine Region im Innenmodus</option>`;
  if (indoor.some((r) => indoorKey(r) === keep)) sel.value = keep;
  $<HTMLButtonElement>("code-load").disabled = !indoor.length;
  codeChoice();
}

/** The button says where the sheets open: here, or on the region's shard;
 * another region clears the shown sheets. */
function codeChoice() {
  const r = regions.find((x) => x.indoor && indoorKey(x) === $<HTMLSelectElement>("code-region").value);
  codesShown = false;
  $("code-sheets").innerHTML = "";
  $("code-sheets").hidden = true;
  $("code-screens").hidden = true;
  $("code-print").hidden = true;
  $("code-sum").textContent = "";
  $("code-load").textContent = r?.base ? `Auf Shard ${r.shard} öffnen ↗` : "Anzeigen";
}

async function loadCodes() {
  const r = regions.find((x) => x.indoor && indoorKey(x) === $<HTMLSelectElement>("code-region").value);
  if (!r) return;
  if (r.base) return void window.open(`${r.base}/indoor.html?region=${encodeURIComponent(r.slug)}`, "_blank", "noopener");
  $("code-sum").textContent = "lädt …";
  const res = await api("", `/api/ops/indoor?region=${encodeURIComponent(r.slug)}`).catch(() => null);
  if (!res?.ok) {
    $("code-sum").textContent = await errText(res);
    return;
  }
  const out = renderIndoor((await res.json()) as IndoorReply, $("code-sheets"), $("code-screen-list"));
  $("code-screens").hidden = !out.screens;
  $("code-sum").textContent = out.text;
  $("code-print").hidden = !out.sheets;
  codesShown = true;
  $("code-sheets").hidden = false;
}

// --- OSM updates ----------------------------------------------------------

const RHYTHM: Record<string, string> = { off: "aus", monthly: "monatlich", quarterly: "vierteljährlich" };
const STATUS: Record<string, [string, string]> = {
  pending: ["wartet auf den Updater", ""],
  running: ["läuft", "ok"],
  done: ["fertig", "ok"],
  waiting: ["wartet auf Freigabe", ""],
  approved: ["freigegeben", "ok"],
  written: ["geschrieben", "ok"],
  failed: ["Fehler", "off"],
  skipped: ["übersprungen", "off"],
};
const updKey = (x: { shard: string; region: string }) => `${x.shard}:${x.region}`;

async function loadUpdates() {
  const lists = await Promise.all(shards.map(async (s) => [s, s.state === "ok" ? await load<{ regions: UpdRegion[]; jobs: UpdJob[] }>(s, "/api/ops/updates") : null] as const));
  updRegions = lists.flatMap(([s, l]) => (l?.regions ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base })));
  updJobs = lists.flatMap(([s, l]) => (l?.jobs ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base }))).sort((a, b) => b.created - a.created);
}

function renderUpdates() {
  const q = $<HTMLInputElement>("upd-q").value.trim().toLowerCase();
  const onlyOn = $<HTMLInputElement>("upd-on").checked;
  const open = new Set(updJobs.filter((j) => ["pending", "running", "waiting", "approved"].includes(j.status)).map(updKey));
  const rows = updRegions.filter((r) => r.active && (!onlyOn || (r.rhythm && r.rhythm !== "off")) && (!q || `${r.name} ${r.region}`.toLowerCase().includes(q)));
  const on = updRegions.filter((r) => r.rhythm && r.rhythm !== "off").length;
  $("upd-sum").textContent = `${rows.length} Regionen · ${on} mit Rhythmus`;
  $("badge-upd").textContent = updJobs.some((j) => j.status === "waiting") ? "!" : "";
  $("upd-regions").innerHTML = rows.length
    ? `<tr><th>Region</th>${many() ? "<th>Shard</th>" : ""}<th>Rhythmus</th><th>zuletzt geprüft</th><th>zuletzt geschrieben</th><th></th></tr>` +
      rows
        .map((r) => {
          const k = esc(updKey(r));
          const sel = `<select data-rhythm="${k}">${["off", "quarterly", "monthly"].map((v) => `<option value="${v}"${(r.rhythm ?? "off") === v ? " selected" : ""}>${RHYTHM[v]}</option>`).join("")}</select>`;
          const btn = open.has(updKey(r)) ? `<span class="slug">Lauf offen</span>` : `<button data-run="${k}">Probelauf jetzt</button>`;
          return `<tr><td>${esc(r.name)}<div class="slug">${esc(r.region)}</div></td>${many() ? `<td>${esc(r.shard)}</td>` : ""}<td>${sel}</td><td>${r.last_check ? when(r.last_check) : "–"}</td><td>${r.last_write ? when(r.last_write) : "–"}</td><td>${btn}</td></tr>`;
        })
        .join("")
    : `<tr><td class="empty">Keine Region${onlyOn ? " mit Rhythmus (Haken weg, um alle zu sehen)" : ""}.</td></tr>`;
  const names = new Map(updRegions.map((r) => [updKey(r), r.name]));
  $("upd-jobs").innerHTML = updJobs.length
    ? `<tr><th>angelegt</th><th>Region</th><th>Stand</th><th>Ergebnis</th><th>von</th><th></th></tr>` +
      updJobs
        .map((j) => {
          const [label, cls] = STATUS[j.status] ?? [j.status, ""];
          const s = j.summary;
          const res = s
            ? `${s.stop ? tag("Grenzwert überschritten", "off") : tag("Grenzwerte eingehalten", "ok")} ${s.gone}/${s.junctions} weg (${s.gone_pct} %), ${s.new} neu · Quellen verloren ${s.sources_lost} (${s.sources_pct} %) · Funde −${s.finds_hidden}/+${s.finds_new}${s.refs_lost ? ` · ${s.refs_lost} Bezüge ohne Nachfolger` : ""} · ${Math.round(s.seconds / 60)} min`
            : esc(j.error ?? "");
          const k = `data-job="${j.id}" data-shard="${esc(j.shard)}"`;
          const rep = s ? `<button data-report="${j.id}" data-shard="${esc(j.shard)}">Bericht</button>` : "";
          const act =
            j.status === "waiting"
              ? `<button ${k} data-act="night">Heute Nacht schreiben</button><button ${k} data-act="now">Jetzt schreiben</button><button ${k} data-act="discard">Verwerfen</button>`
              : j.status === "approved"
                ? `<span class="slug">${j.run_now ? "schreibt beim nächsten Durchgang" : "schreibt nachts 02–05 Uhr"} · ${esc(j.approved_by ?? "")}</span><button ${k} data-act="discard">Verwerfen</button>`
                : "";
          return `<tr><td>${when(j.created)}</td><td>${esc(names.get(updKey(j)) ?? j.region)}${many() ? `<div class="slug">${esc(j.shard)}</div>` : ""}</td><td>${tag(label, cls)}</td><td class="text">${res}</td><td>${esc(j.by)}</td><td class="links">${rep}${act}</td></tr>`;
        })
        .join("")
    : `<tr><td class="empty">Noch keine Läufe.</td></tr>`;
}

$("upd-regions").addEventListener("change", async (e) => {
  const sel = (e.target as HTMLElement).closest<HTMLSelectElement>("select[data-rhythm]");
  if (!sel) return;
  const r = updRegions.find((x) => updKey(x) === sel.dataset.rhythm);
  if (!r) return;
  sel.disabled = true;
  const res = await api(r.base, "/api/ops/updates", { method: "POST", body: JSON.stringify({ region: r.region, rhythm: sel.value }) }).catch(() => null);
  sel.disabled = false;
  if (!res?.ok) return void msg(`Update ${r.region}: ${await errText(res)}`);
  r.rhythm = sel.value;
  msg("");
});

$("upd-regions").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-run]");
  if (!b) return;
  const r = updRegions.find((x) => updKey(x) === b.dataset.run);
  if (!r) return;
  b.disabled = true;
  const res = await api(r.base, "/api/ops/updates", { method: "POST", body: JSON.stringify({ region: r.region, run: true }) }).catch(() => null);
  if (!res?.ok) {
    b.disabled = false;
    return void msg(`Probelauf ${r.region}: ${await errText(res)}`);
  }
  msg(`Probelauf für ${r.name} angelegt; der Updater nimmt ihn beim nächsten Durchgang.`);
  await loadUpdates();
  renderUpdates();
});

$("upd-jobs").addEventListener("click", async (e) => {
  const a = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-act]");
  if (a) return void decideJob(a);
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-report]");
  if (!b) return;
  const s = shards.find((x) => x.shard === b.dataset.shard);
  if (!s) return;
  const j = await load<{ region: string; report: string | null }>(s, `/api/ops/updates/${b.dataset.report}`);
  $("upd-report").hidden = !j;
  if (!j) return void drawShards();
  $("upd-report-title").textContent = `Bericht ${j.region}`;
  $("upd-report-text").textContent = j.report ?? "(kein Bericht)";
  $("upd-report").scrollIntoView({ behavior: "smooth" });
});

/** Approve (tonight or now) or discard an update job. */
async function decideJob(b: HTMLButtonElement) {
  const j = updJobs.find((x) => String(x.id) === b.dataset.job && x.shard === b.dataset.shard);
  if (!j) return;
  const name = updRegions.find((r) => updKey(r) === updKey(j))?.name ?? j.region;
  const act = b.dataset.act;
  const stop = j.summary?.stop ? "\n\nACHTUNG: Ein Grenzwert ist überschritten. Freigeben schreibt trotzdem." : "";
  const ask =
    act === "discard"
      ? `Update für ${name} verwerfen? Der nächste Probelauf kommt nach dem Rhythmus.`
      : act === "now"
        ? `Update für ${name} beim nächsten Durchgang schreiben (höchstens 15 Minuten)? Danach wird die Region neu geladen: Wer dort spielt, wird kurz getrennt. Die Zahlen werden beim Schreiben neu gerechnet.${stop}`
        : `Update für ${name} heute Nacht (02–05 Uhr Ortszeit) schreiben? Danach Neuladen und Basiskarte automatisch. Die Zahlen werden beim Schreiben neu gerechnet.${stop}`;
  if (!confirm(ask)) return;
  b.disabled = true;
  const body = act === "discard" ? { discard: j.id } : { approve: j.id, now: act === "now" };
  const res = await api(j.base, "/api/ops/updates", { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    b.disabled = false;
    return void msg(`Update ${j.region}: ${await errText(res)}`);
  }
  msg("");
  await loadUpdates();
  renderUpdates();
}

// --- maintenance ----------------------------------------------------------

const NOTE: Record<string, string> = { move: "Umzug", update: "Server-Update", database: "Datenbank" };
const when = (t: number) => new Date(t * 1000).toLocaleString("de-DE", { weekday: "short", day: "numeric", month: "numeric", hour: "2-digit", minute: "2-digit" });

async function loadMaint() {
  const lists = await Promise.all(shards.map(async (s) => [s, s.state === "ok" ? await load<{ items: Maint[] }>(s, "/api/ops/maintenance") : null] as const));
  maint = lists.flatMap(([s, l]) => (l?.items ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base })));
}

function renderMaint() {
  const sel = $<HTMLSelectElement>("maint-shard");
  const keep = sel.value;
  const up = shards.filter((s) => s.state === "ok");
  sel.innerHTML = (up.length > 1 ? `<option value="*">alle Shards</option>` : "") + up.map((s) => `<option value="${esc(s.shard)}">Shard ${esc(s.shard)}</option>`).join("");
  if ([...sel.options].some((o) => o.value === keep)) sel.value = keep;
  const now = Date.now() / 1000;
  const open = (m: Maint) => !m.cancelled && m.ends > now;
  const rows = [...maint].sort((a, b) => Number(open(b)) - Number(open(a)) || (open(a) ? a.starts - b.starts : b.starts - a.starts));
  $("badge-maint").textContent = maint.some((m) => open(m) && m.starts - now < 48 * 3600) ? "●" : "";
  $("maint").innerHTML = rows.length
    ? `<tr><th>Beginn</th><th>Ende</th>${many() ? "<th>Shard</th>" : ""}<th>Zusatz</th><th>Stand</th><th>von</th><th></th></tr>` +
      rows
        .map((m) => {
          const state = m.cancelled ? tag("abgesagt", "off") : m.ends <= now ? tag("vorbei") : m.starts <= now ? tag("läuft", "ok") : m.starts - now < 48 * 3600 ? tag("angekündigt", "ok") : tag("geplant");
          const by = m.cancelled_by ? `${esc(m.by)}<div class="slug">abgesagt: ${esc(m.cancelled_by)}</div>` : esc(m.by);
          const btn = open(m) ? `<button data-cancel="${m.id}" data-shard="${esc(m.shard)}">Absagen</button>` : "";
          return `<tr${open(m) ? "" : ' class="done"'}><td>${when(m.starts)}</td><td>${when(m.ends)}</td>${many() ? `<td>${esc(m.shard)}</td>` : ""}<td>${esc(m.note ? (NOTE[m.note] ?? m.note) : "")}</td><td>${state}</td><td>${by}</td><td>${btn}</td></tr>`;
        })
        .join("")
    : `<tr><td class="empty">Keine Wartungsfenster.</td></tr>`;
}

/** Default: the night after next, 03:00 (48 h lead). */
{
  const d = new Date(Date.now() + 2 * 86400_000);
  d.setHours(3, 0, 0, 0);
  const p = (n: number) => String(n).padStart(2, "0");
  $<HTMLInputElement>("maint-start").value = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T03:00`;
}

$("maint-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const start = new Date($<HTMLInputElement>("maint-start").value).getTime() / 1000;
  const minutes = Number($<HTMLInputElement>("maint-min").value);
  const note = $<HTMLSelectElement>("maint-note").value || null;
  const target = $<HTMLSelectElement>("maint-shard").value;
  const to = shards.filter((s) => s.state === "ok" && (target === "*" || s.shard === target));
  if (!Number.isFinite(start) || !to.length) return;
  if (start - Date.now() / 1000 < 24 * 3600 && !confirm("Weniger als 24 Stunden Vorlauf: Spielende sehen den Hinweis nur kurz. Trotzdem eintragen?")) return;
  if (!confirm(`Wartung ${when(start)}, ${minutes} min, ${to.map((s) => s.shard).join(", ")} eintragen?`)) return;
  const fails: string[] = [];
  for (const s of to) {
    const res = await api(s.base, "/api/ops/maintenance", { method: "POST", body: JSON.stringify({ starts: start, minutes, note }) }).catch(() => null);
    if (!res?.ok) fails.push(`${s.shard}: ${await errText(res)}`);
  }
  msg(fails.join(" · "));
  await loadMaint();
  renderMaint();
});

$("maint").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-cancel]");
  if (!b) return;
  const s = shards.find((x) => x.shard === b.dataset.shard);
  if (!s || !confirm("Wartungsfenster absagen? Der Hinweis verschwindet in der App innerhalb weniger Minuten.")) return;
  b.disabled = true;
  const res = await api(s.base, "/api/ops/maintenance", { method: "POST", body: JSON.stringify({ cancel: Number(b.dataset.cancel) }) }).catch(() => null);
  if (!res?.ok) {
    b.disabled = false;
    return void msg(`Wartung ${s.shard}: ${await errText(res)}`);
  }
  await loadMaint();
  renderMaint();
});

// --- tabs and start -------------------------------------------------------

async function show(tab: string) {
  put(TAB_KEY, tab);
  for (const b of document.querySelectorAll<HTMLButtonElement>("#tabs button")) b.classList.toggle("on", b.dataset.tab === tab);
  for (const s of document.querySelectorAll<HTMLElement>(".tab")) s.hidden = s.id !== `tab-${tab}`;
  $("code-sheets").hidden = tab !== "codes" || !codesShown;
  if (tab === "wuensche" && !wishes) {
    $("wish-sum").textContent = "lädt …";
    await loadWishes();
    renderWishes();
  }
}

type RegionsReply = { shard: string; regions: Region[]; peers?: { shard: string; base: string }[] };

async function start() {
  msg("");
  if (!(await checkAccess())) return;
  const play = await playUrls();
  const here: Shard = { shard: "", base: "", play: "", state: "wait" };
  shards = [here];
  const first = await load<RegionsReply>(here, "/api/ops/regions");
  if (first) {
    here.shard = first.shard;
    here.state = "ok";
    shards.push(...(first.peers ?? []).map((p) => ({ shard: p.shard, base: p.base, play: play.get(p.shard) ?? p.base, state: "wait" })));
  }
  drawShards();
  const replies = await Promise.all(shards.map(async (s) => [s, s === here ? first : await load<RegionsReply>(s, "/api/ops/regions")] as const));
  const up = (s: Shard) => s.state === "wait" || s.state === "ok";
  const [fbs, pends] = await Promise.all([
    Promise.all(shards.map(async (s) => [s, up(s) ? await load<Feedback[]>(s, "/api/feedback") : null] as const)),
    Promise.all(shards.map(async (s) => [s, up(s) ? await load<{ media: PendMedia[]; mottos: PendMotto[] }>(s, "/api/ops/pending") : null] as const)),
  ]);
  for (const [s, r] of replies) if (r && s.state === "wait") s.state = "ok";
  regions = replies
    .flatMap(([s, r]) => (r?.regions ?? []).map((x) => ({ ...x, shard: s.shard || r?.shard || "", base: s.base })))
    // Busiest first, then by name.
    .sort((a, b) => b.players_7d - a.players_7d || b.players_today - a.players_today || a.name.localeCompare(b.name));
  feedback = fbs.flatMap(([s, f]) => (f ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base }))).sort((a, b) => b.day.localeCompare(a.day));
  pendMedia = pends.flatMap(([s, p]) => (p?.media ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base }))).sort((a, b) => a.created - b.created);
  pendMottos = pends.flatMap(([s, p]) => (p?.mottos ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base })));
  $("shard-name").textContent = many() ? `· ${shards.length} Shards` : first ? `· Shard ${first.shard}` : "";
  drawShards();
  renderRegions();
  feedbackRegions();
  renderFeedback();
  renderPending();
  renderCodes();
  await Promise.all([loadMaint(), loadUpdates()]);
  renderMaint();
  renderUpdates();
  wishes = null;
  await show(get(TAB_KEY) || "regionen");
}

$("filters").addEventListener("submit", (e) => e.preventDefault());
for (const b of document.querySelectorAll<HTMLButtonElement>("#tabs button")) b.onclick = () => void show(b.dataset.tab!);
$("region-q").oninput = renderRegions;
$("region-active").onchange = renderRegions;
$("fb-region").onchange = renderFeedback;
$("fb-text").onchange = renderFeedback;
$("fb-open").onchange = renderFeedback;
$("wish-filter").onchange = renderWishes;
$("upd-q").oninput = renderUpdates;
$("upd-on").onchange = renderUpdates;
$("code-region").onchange = codeChoice;
$("code-load").onclick = () => void loadCodes();
$("code-print").onclick = () => print();
const tokenInput = $<HTMLInputElement>("token");
tokenInput.value = token;
tokenInput.onchange = () => {
  token = tokenInput.value.trim();
  put(TOKEN_KEY, token);
  void start();
};
$("logout").onclick = () => {
  if (!token && logoutTo?.logout) return void location.assign(logoutTo.logout);
  token = "";
  tokenInput.value = "";
  put(TOKEN_KEY, "");
  void start();
};
void start();
