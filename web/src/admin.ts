// Admin page (admin.html): one place for the operator across regions and
// shards — regions (`/api/ops/regions`), feedback (`/api/feedback`, mark
// done), "play here" wishes (`/api/wishes`) and indoor QR sheets
// (indoor.html). Needs admin for all regions: the ops token, or a login
// (opsauth.ts) in the group `<prefix>admin`. The other shards come from
// this server's `peers` (ELEMENTS_ADMIN_PEERS, server admin.rs): the page asks
// them itself with the same login (cookie) or token.
import { authHeaders, getAccess, type Access } from "./opsauth";

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

function renderCodes() {
  const indoor = regions.filter((r) => r.indoor);
  $("codes").innerHTML = indoor.length
    ? indoor
        .map((r) => `<li><a href="${r.base}/indoor.html?region=${encodeURIComponent(r.slug)}" target="_blank">${esc(r.name)}</a> <span class="slug">${esc(r.slug)}${many() ? ` · ${esc(r.shard)}` : ""}${r.active ? "" : " · aus"}${r.listed ? "" : " · ungelistet"}</span></li>`)
        .join("")
    : "<li>Keine Region im Innenmodus.</li>";
}

// --- tabs and start -------------------------------------------------------

async function show(tab: string) {
  put(TAB_KEY, tab);
  for (const b of document.querySelectorAll<HTMLButtonElement>("#tabs button")) b.classList.toggle("on", b.dataset.tab === tab);
  for (const s of document.querySelectorAll<HTMLElement>(".tab")) s.hidden = s.id !== `tab-${tab}`;
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
  const fbs = await Promise.all(shards.map(async (s) => [s, s.state === "wait" || s.state === "ok" ? await load<Feedback[]>(s, "/api/feedback") : null] as const));
  for (const [s, r] of replies) if (r && s.state === "wait") s.state = "ok";
  regions = replies
    .flatMap(([s, r]) => (r?.regions ?? []).map((x) => ({ ...x, shard: s.shard || r?.shard || "", base: s.base })))
    // Busiest first, then by name.
    .sort((a, b) => b.players_7d - a.players_7d || b.players_today - a.players_today || a.name.localeCompare(b.name));
  feedback = fbs.flatMap(([s, f]) => (f ?? []).map((x) => ({ ...x, shard: s.shard, base: s.base }))).sort((a, b) => b.day.localeCompare(a.day));
  $("shard-name").textContent = many() ? `· ${shards.length} Shards` : first ? `· Shard ${first.shard}` : "";
  drawShards();
  renderRegions();
  feedbackRegions();
  renderFeedback();
  renderCodes();
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
