// Admin page (admin.html): one place for the operator across regions —
// regions of this shard (`/api/ops/regions`), feedback (`/api/feedback`),
// "play here" wishes (`/api/wishes`) and indoor QR sheets (indoor.html).
// Needs admin for all regions: the ops token, or a login (opsauth.ts) in the
// group `<prefix>admin`. Other shards: links to their page (same token).
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
type Feedback = { id?: number; region: string; day: string; mood: number; text: string | null; lang: string | null; version: string | null };
type Wish = { cell: string; wishes: number; last: string; center: [number, number] | null };
type Listed = { slug: string; name: string; outline?: { type: string; coordinates: number[][][] | number[][][][] } };

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
let regions: Region[] = [];
let feedback: Feedback[] = [];
const api = (path: string, init: RequestInit = {}) => fetch(path, { ...init, headers: { ...(init.headers ?? {}), ...authHeaders(token) } });
const msg = (s: string) => ($("msg").textContent = s);

async function load(path: string): Promise<unknown | null> {
  const res = await api(path).catch(() => null);
  if (!res?.ok) {
    msg(`${path}: ${res ? `Fehler ${res.status}` : "kein Netz"}`);
    return null;
  }
  return res.json();
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
  logoutTo = a as Access | null;
  return ok;
}
let logoutTo: Access | null = null;

// --- shards ---------------------------------------------------------------

async function shards(current: string) {
  const res = await fetch("/api/marble/shards.json").catch(() => null);
  const j = res?.ok ? ((await res.json().catch(() => null)) as { shards?: { shard: string; play: string }[] } | null) : null;
  const nav = $("shards");
  nav.replaceChildren();
  for (const s of j?.shards ?? []) {
    const a = Object.assign(document.createElement("a"), { textContent: s.shard, href: s.shard === current ? "#" : `${s.play}/admin.html` });
    a.classList.toggle("on", s.shard === current);
    nav.append(a);
  }
}

// --- regions --------------------------------------------------------------

const tag = (text: string, cls = "") => `<span class="tag ${cls}">${esc(text)}</span>`;

function renderRegions() {
  const q = $<HTMLInputElement>("region-q").value.trim().toLowerCase();
  const onlyActive = $<HTMLInputElement>("region-active").checked;
  const rows = regions.filter((r) => (!onlyActive || r.active) && (!q || `${r.name} ${r.slug} ${r.rules ?? ""}`.toLowerCase().includes(q)));
  const sum = (k: "players_today" | "players_7d") => rows.reduce((n, r) => n + r[k], 0);
  $("region-sum").textContent = `${rows.length} von ${regions.length} · heute ${sum("players_today")} Spielende, 7 Tage ${sum("players_7d")}`;
  $("regions").innerHTML =
    `<tr><th>Region</th><th>Stand</th><th>Regelsatz</th><th class="num">heute</th><th class="num">7 Tage</th><th class="num">Rückm. 30 T</th><th>Öffnen</th><th></th></tr>` +
    rows
      .map((r) => {
        const s = encodeURIComponent(r.slug);
        const state = [r.active ? tag("aktiv", "ok") : tag("aus", "off"), r.listed ? "" : tag("ungelistet"), r.loaded ? tag("geladen") : "", r.cell !== "main" ? tag(`Zelle ${r.cell}`) : ""].join("");
        const links = [`<a href="/?region=${s}" target="_blank">Spiel</a>`, `<a href="/dashboard.html?region=${s}" target="_blank">Dashboard</a>`, `<a href="/editor.html?region=${s}" target="_blank">Redaktion</a>`, r.indoor ? `<a href="/indoor.html?region=${s}" target="_blank">Aushänge</a>` : ""].join("");
        return `<tr><td>${esc(r.name)}<div class="slug">${esc(r.slug)} · ${esc(r.language)}</div></td><td>${state}</td><td>${esc(r.rules ?? "Standard")}${r.indoor ? ` ${tag("drinnen", "in")}` : ""}</td><td class="num">${r.players_today}</td><td class="num">${r.players_7d}</td><td class="num">${r.feedback_30d || ""}</td><td class="links">${links}</td><td>${r.active ? `<button data-reload="${esc(r.slug)}">Neu laden</button>` : ""}</td></tr>`;
      })
      .join("");
}

$("regions").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-reload]");
  if (!b) return;
  const slug = b.dataset.reload!;
  if (!confirm(`${slug} neu laden? Wer gerade spielt, wird kurz getrennt.`)) return;
  b.disabled = true;
  const res = await api(`/api/ops/reload?region=${encodeURIComponent(slug)}`, { method: "POST" }).catch(() => null);
  b.textContent = res?.ok ? "geladen ✓" : `Fehler ${res?.status ?? "Netz"}`;
});

// --- feedback -------------------------------------------------------------

const FACE: Record<number, string> = { 1: "☹️", 2: "😐", 3: "😊" };

function renderFeedback() {
  const reg = $<HTMLSelectElement>("fb-region").value;
  const withText = $<HTMLInputElement>("fb-text").checked;
  const names = new Map(regions.map((r) => [r.slug, r.name]));
  const rows = feedback.filter((f) => (!reg || f.region === reg) && (!withText || f.text?.trim()));
  $("fb-sum").textContent = `${rows.length} von ${feedback.length} (die letzten 500)`;
  $("feedback").innerHTML =
    `<tr><th>Tag</th><th></th><th>Region</th><th>Text</th><th>Sprache</th><th>Version</th></tr>` +
    rows
      .map((f) => `<tr><td>${esc(f.day)}</td><td class="mood" title="${f.mood}">${FACE[f.mood] ?? f.mood}</td><td>${esc(names.get(f.region) ?? f.region)}</td><td class="text">${esc(f.text ?? "")}</td><td>${esc(f.lang ?? "")}</td><td class="slug">${esc(f.version ?? "")}</td></tr>`)
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
  // Badge: feedback with text in the last 7 days.
  const since = new Date(Date.now() - 7 * 864e5).toISOString().slice(0, 10);
  const recent = feedback.filter((f) => f.day >= since && f.text?.trim()).length;
  $("badge-fb").textContent = recent ? String(recent) : "";
}

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

async function renderWishes() {
  const [w, l] = await Promise.all([load("/api/wishes") as Promise<Wish[] | null>, fetch("/api/regions").then((r) => r.json()).catch(() => [])]);
  const listed: Listed[] = Array.isArray(l) ? l : (l?.regions ?? []);
  if (!w) return;
  $("wishes").innerHTML =
    `<tr><th class="num">Wünsche</th><th>zuletzt</th><th>Ort</th><th>in Region</th><th>Zelle</th></tr>` +
    w
      .map((c) => {
        const at = c.center;
        const map = at ? `<a href="https://www.openstreetmap.org/?mlat=${at[1].toFixed(5)}&mlon=${at[0].toFixed(5)}#map=11/${at[1].toFixed(4)}/${at[0].toFixed(4)}" target="_blank">${at[1].toFixed(3)}, ${at[0].toFixed(3)}</a>` : "";
        const reg = at ? listed.find((r) => inside(at, r.outline)) : undefined;
        return `<tr><td class="num">${c.wishes}</td><td>${esc(c.last)}</td><td>${map}</td><td>${reg ? esc(reg.name) : ""}</td><td class="slug">${esc(c.cell)}</td></tr>`;
      })
      .join("");
}

// --- QR codes -------------------------------------------------------------

function renderCodes() {
  const indoor = regions.filter((r) => r.indoor);
  $("codes").innerHTML = indoor.length
    ? indoor.map((r) => `<li><a href="/indoor.html?region=${encodeURIComponent(r.slug)}" target="_blank">${esc(r.name)}</a> <span class="slug">${esc(r.slug)}${r.active ? "" : " · aus"}${r.listed ? "" : " · ungelistet"}</span></li>`).join("")
    : "<li>Keine Region im Innenmodus auf diesem Shard.</li>";
}

// --- tabs and start -------------------------------------------------------

const loaded = new Set<string>();
async function show(tab: string) {
  put(TAB_KEY, tab);
  for (const b of document.querySelectorAll<HTMLButtonElement>("#tabs button")) b.classList.toggle("on", b.dataset.tab === tab);
  for (const s of document.querySelectorAll<HTMLElement>(".tab")) s.hidden = s.id !== `tab-${tab}`;
  if (tab === "wuensche" && !loaded.has(tab)) {
    loaded.add(tab);
    await renderWishes();
  }
}

async function start() {
  msg("");
  if (!(await checkAccess())) return;
  const [r, f] = await Promise.all([load("/api/ops/regions") as Promise<{ shard: string; regions: Region[] } | null>, load("/api/feedback") as Promise<Feedback[] | null>]);
  // Busiest first, then by name.
  regions = (r?.regions ?? []).sort((a, b) => b.players_7d - a.players_7d || b.players_today - a.players_today || a.name.localeCompare(b.name));
  feedback = f ?? [];
  $("shard-name").textContent = r ? `· Shard ${r.shard}` : "";
  void shards(r?.shard ?? "");
  renderRegions();
  feedbackRegions();
  renderFeedback();
  renderCodes();
  loaded.clear();
  await show(get(TAB_KEY) || "regionen");
}

for (const b of document.querySelectorAll<HTMLButtonElement>("#tabs button")) b.onclick = () => void show(b.dataset.tab!);
$("region-q").oninput = renderRegions;
$("region-active").onchange = renderRegions;
$("fb-region").onchange = renderFeedback;
$("fb-text").onchange = renderFeedback;
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
