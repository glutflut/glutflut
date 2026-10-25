import * as maplibregl from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { basemapStyle, localLabels } from "./basemap";
import { applyStatic, lang, langChosen, langSwitch, loadCity, t, tOr, type Key } from "./i18n";
import "./version";
import { TileLayer, loadMeta, type Meta } from "./tiles";
import { listRegions, region, setRegion, withRegion } from "./region";
import { COLORS, circle, distM } from "./geo";
import { $ } from "./page";
import { KEYS, load as loadKey, save as saveKey } from "./store";
import { authHeaders, getAccess, type Access } from "./opsauth";

maplibregl.setWorkerUrl(workerUrl);

// Sequential blue ramp (steps 200..650), light -> dark = low -> high.
const RAMP = ["#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#104281"];
const CATS = ["voll", "ruhig", "laut", "dunkel", "kalt", "einladend"];
const catLabel = (c: string) => (CATS.includes(c) ? t(`cat.${c}` as Key) : c);

interface Summary {
  days: number;
  /** Set when one day was requested (the demo day). */
  day: string | null;
  demo_day: string;
  k: number;
  /** [edge, person-quarter-hours, dwell s, of which near an active hotspot s] */
  dwell: [number, number, number, number][];
  hourly: [number, number][];
  perception: [number, string, number][];
  /** [node, category, local hour, n]; only place-hours with >= min_ratings */
  perception_hourly: [number, string, number, number][];
  /** [local hour, category, n] over all places; only hours with >= min_ratings */
  perception_by_hour: [number, string, number][];
  experiment: { active: boolean; slots: number; nearby: number; reached: number; reached_nearby: number }[];
  hourly_edges: [number, number, number][];
  zones: { id: string; name: string; lat: number; lon: number; focus?: boolean; radius_m?: number }[];
  flows: [string, string, number][];
  /** day, players who count, newcomers who left before counting, flagged (implausible tracks), new identities, of those claimed within five minutes */
  daily: [string, number, number, number, number?, number?][];
  /** Person-days per answer, one marginal each; null = hidden (below k). */
  profiles: { age: [string, number | null][]; local: [string, number | null][] };
  /** Places need this many ratings to be published. */
  min_ratings: number;
  suppressed: { kept: number; dropped: number };
  /** Per H3 resolution ("8", "9", "10"). */
  hexes: Record<string, HexRes>;
  /** Rated places: node id -> [lon, lat]. */
  places: Record<string, [number, number]>;
  /** Extent of the segments with dwell data [w, s, e, n], if any. */
  dwell_bounds: [number, number, number, number] | null;
  /** [day, seen that day, new that day, played on 2+ days, played in the last 30 days] */
  growth?: [string, number, number, number, number][];
  /** Most persons in the region in a quarter hour: [persons, window start unix s]. */
  peak?: [number, number] | null;
  /** Short-link hits in the period (all regions): [code, hits]. */
  links?: [string, number][];
}

/** Shown when the data was refused or failed: empty map and charts. */
const EMPTY: Summary = {
  days: 0,
  day: null,
  demo_day: "",
  k: 5,
  dwell: [],
  hourly: [],
  perception: [],
  perception_hourly: [],
  perception_by_hour: [],
  experiment: [],
  hourly_edges: [],
  zones: [],
  flows: [],
  daily: [],
  profiles: { age: [], local: [] },
  min_ratings: 3,
  suppressed: { kept: 0, dropped: 0 },
  hexes: {},
  places: {},
  dwell_bounds: null,
};

interface HexRes {
  cells: GeoJSON.FeatureCollection<GeoJSON.Polygon, { cell: string; persons: number; dwell_s: number; hotspot_dwell_s: number }>;
  /** [feature index, local hour, persons] */
  hourly: [number, number, number][];
  kept: number;
  dropped: number;
}

const AGES = ["u30", "30_59", "60p"];
const ageLabel = (a: string) => t(`dash.age.${a}` as Key);

/** "Verweildauer x min", plus the hotspot part if any. */
const dwellText = (d: number, hot: number) => {
  const min = fmt.format(Math.round(d / 60));
  return hot > 0 ? t("dash.dwell.time.hot", { min, hot: fmt.format(Math.round(hot / 60)) }) : t("dash.dwell.time", { min });
};

/** H3 edge length in m per resolution. */
const HEX_M: Record<string, number> = { "8": 460, "9": 170, "10": 65, "11": 25 };
const hexSize = (res: string) => t("dash.hex.size", { m: fmt.format(HEX_M[res]) });
/** "17–18 Uhr" */
const hourRange = (h: number) => t("dash.hour.range", { h, h2: h + 1 });

type Mode = "dwell" | "hexes" | "ratings" | "flows" | "areas";

/** `/api/areas`: held area per side and hour, blocks held longest. */
interface Areas {
  days: number;
  /** [unix hour, fire ha, water ha, fire with players, water with players, fire peak, water peak, minutes] */
  hours: [number, number, number, number, number, number, number, number][];
  /** Properties: minutes held by fire (f) and water (w), area (ha). */
  blocks: GeoJSON.FeatureCollection<GeoJSON.Geometry, { id: number; f: number; w: number; ha: number }>;
  total_ha?: number;
}
const NO_AREAS: Areas = { days: 0, hours: [], blocks: { type: "FeatureCollection", features: [] } };
/** The list below the map needs a few more ratings than the map. */
const MIN_RATINGS_LIST = 5;
/** Share breaks for the 5 ramp steps. */
const SHARE_BREAKS = [0.2, 0.4, 0.6, 0.8];
const FLOW_COLOR = "#256abf";

// Number and date formats follow the UI language (which loadCity may switch).
const LOCALE: Record<string, string> = { de: "de-DE", en: "en-GB", fr: "fr-FR", es: "es-CO", pl: "pl-PL", it: "it-IT", pt: "pt-BR" };
const locale = () => LOCALE[lang] ?? "de-DE";
let fmt = new Intl.NumberFormat(locale());
let pct = new Intl.NumberFormat(locale(), { style: "percent", maximumFractionDigits: 1 });
function setFormats() {
  fmt = new Intl.NumberFormat(locale());
  pct = new Intl.NumberFormat(locale(), { style: "percent", maximumFractionDigits: 1 });
}
const SVG = "http://www.w3.org/2000/svg";

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>, text?: string) {
  const e = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  if (text !== undefined) e.textContent = text;
  return e;
}

// Tooltip: value first, label second; text only.
const tip = $("tip");
function showTip(ev: PointerEvent | FocusEvent, value: string, label: string) {
  tip.replaceChildren(Object.assign(document.createElement("strong"), { textContent: value }), document.createTextNode(label));
  tip.hidden = false;
  const r = (ev.target as Element).getBoundingClientRect();
  const x = "clientX" in ev ? ev.clientX : r.left + r.width / 2;
  const y = "clientY" in ev ? ev.clientY : r.top;
  tip.style.left = `${Math.min(x + 12, innerWidth - 180)}px`;
  tip.style.top = `${y - 44}px`;
}
const hideTip = () => (tip.hidden = true);

/** Vertical bars from one baseline; 4px rounded tops, capped width. */
function columns(host: HTMLElement, data: { label: string; value: number; tip: string; tipLabel?: string }[], opts: { every?: number; fmtValue?: (v: number) => string } = {}) {
  host.replaceChildren();
  if (!data.length || data.every((d) => d.value === 0)) {
    host.append(Object.assign(document.createElement("div"), { className: "empty", textContent: t("dash.empty") }));
    return;
  }
  const W = Math.max(host.clientWidth, 280), H = 200, L = 36, B = 22, T = 18;
  const max = Math.max(...data.map((d) => d.value));
  const step = niceStep(max);
  const top = Math.ceil(max / step) * step;
  const svg = el("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
  for (let v = 0; v <= top; v += step) {
    const y = H - B - ((H - B - T) * v) / top;
    svg.append(el("line", { x1: L, x2: W, y1: y, y2: y, class: v === 0 ? "base" : "grid" }));
    svg.append(el("text", { x: L - 6, y: y + 4, "text-anchor": "end" }, opts.fmtValue ? opts.fmtValue(v) : fmt.format(v)));
  }
  const band = (W - L) / data.length;
  const bw = Math.min(24, band - 2);
  data.forEach((d, i) => {
    const h = ((H - B - T) * d.value) / top;
    const x = L + i * band + (band - bw) / 2;
    const y = H - B - h;
    const r = Math.min(4, h);
    const path = `M${x},${H - B}V${y + r}Q${x},${y} ${x + r},${y}H${x + bw - r}Q${x + bw},${y} ${x + bw},${y + r}V${H - B}Z`;
    const bar = el("path", { d: path, class: "bar", tabindex: 0 });
    bar.addEventListener("pointermove", (e) => showTip(e, d.tip, d.tipLabel ?? d.label));
    bar.addEventListener("focus", (e) => showTip(e, d.tip, d.tipLabel ?? d.label));
    bar.addEventListener("pointerleave", hideTip);
    bar.addEventListener("blur", hideTip);
    svg.append(bar);
    if (!opts.every || i % opts.every === 0) {
      svg.append(el("text", { x: x + bw / 2, y: H - 6, "text-anchor": "middle" }, d.label));
    }
  });
  // Label the extreme only.
  const iMax = data.findIndex((d) => d.value === max);
  const xm = L + iMax * band + band / 2;
  const ym = H - B - ((H - B - T) * max) / top;
  svg.append(el("text", { x: xm, y: ym - 5, "text-anchor": "middle", class: "val" }, opts.fmtValue ? opts.fmtValue(max) : fmt.format(max)));
  host.append(svg);
}

/** Horizontal bars with the value at the tip. */
function hbars(host: HTMLElement, data: { label: string; value: number; valueText: string; tip: string }[]) {
  host.replaceChildren();
  if (!data.length) {
    host.append(Object.assign(document.createElement("div"), { className: "empty", textContent: t("dash.empty") }));
    return;
  }
  const W = Math.max(host.clientWidth, 280), rowH = 32, L = 110, R = 70;
  const H = data.length * rowH + 4;
  const max = Math.max(...data.map((d) => d.value), 1e-9);
  const svg = el("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
  svg.append(el("line", { x1: L, x2: L, y1: 0, y2: H, class: "base" }));
  data.forEach((d, i) => {
    const y = i * rowH + (rowH - 20) / 2;
    const w = Math.max(((W - L - R) * d.value) / max, d.value > 0 ? 2 : 0);
    const r = Math.min(4, w);
    const path = `M${L},${y}H${L + w - r}Q${L + w},${y} ${L + w},${y + r}V${y + 20 - r}Q${L + w},${y + 20} ${L + w - r},${y + 20}H${L}Z`;
    const bar = el("path", { d: path, class: "bar", tabindex: 0 });
    bar.addEventListener("pointermove", (e) => showTip(e, d.tip, d.label));
    bar.addEventListener("focus", (e) => showTip(e, d.tip, d.label));
    bar.addEventListener("pointerleave", hideTip);
    bar.addEventListener("blur", hideTip);
    svg.append(el("text", { x: L - 8, y: y + 14, "text-anchor": "end", class: "val" }, d.label));
    svg.append(bar);
    svg.append(el("text", { x: L + w + 6, y: y + 14, class: "val" }, d.valueText));
  });
  host.append(svg);
}

/** Lines over time from one baseline; x as unix seconds. */
function lines(host: HTMLElement, xs: number[], series: { values: number[]; color: string; thin?: boolean; name: string }[], fmtV: (v: number) => string, fmtX: (x: number) => string) {
  host.replaceChildren();
  if (!xs.length || series.every((s) => s.values.every((v) => v === 0))) {
    host.append(Object.assign(document.createElement("div"), { className: "empty", textContent: t("dash.empty") }));
    return;
  }
  const W = Math.max(host.clientWidth, 280), H = 200, L = 40, B = 22, T = 14, R = 8;
  const max = Math.max(...series.flatMap((s) => s.values), 1e-9);
  const step = niceStep(max);
  const top = Math.ceil(max / step) * step;
  const [x0, x1] = [xs[0], xs[xs.length - 1]];
  const sx = (x: number) => L + ((W - L - R) * (x - x0)) / Math.max(1, x1 - x0);
  const sy = (v: number) => H - B - ((H - B - T) * v) / top;
  const svg = el("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
  for (let v = 0; v <= top + 1e-9; v += step) {
    svg.append(el("line", { x1: L, x2: W - R, y1: sy(v), y2: sy(v), class: v === 0 ? "base" : "grid" }));
    svg.append(el("text", { x: L - 6, y: sy(v) + 4, "text-anchor": "end" }, fmtV(v)));
  }
  const ticks = x1 > x0 ? 5 : 1;
  for (let i = 0; i < ticks; i++) {
    const x = x0 + ((x1 - x0) * i) / 4;
    svg.append(el("text", { x: sx(x), y: H - 6, "text-anchor": i === 0 ? "start" : i === 4 ? "end" : "middle" }, fmtX(x)));
  }
  for (const s of series) {
    // A gap of more than two hours breaks the line (region asleep).
    let d = "";
    s.values.forEach((v, i) => {
      d += `${i === 0 || xs[i] - xs[i - 1] > 7200 ? "M" : "L"}${sx(xs[i]).toFixed(1)},${sy(v).toFixed(1)}`;
    });
    svg.append(el("path", { d, class: `line${s.thin ? " thin" : ""}`, stroke: s.color }));
    // A lone hour has no line: a dot.
    s.values.forEach((v, i) => {
      const alone = (i === 0 || xs[i] - xs[i - 1] > 7200) && (i === xs.length - 1 || xs[i + 1] - xs[i] > 7200);
      if (alone && !s.thin) svg.append(el("circle", { cx: sx(xs[i]), cy: sy(v), r: 3, fill: s.color }));
    });
  }
  // Hover: the nearest hour, all series.
  const hit = el("rect", { x: L, y: T, width: W - L - R, height: H - B - T, fill: "transparent" });
  hit.addEventListener("pointermove", (e) => {
    const r = svg.getBoundingClientRect();
    const x = x0 + (((e.clientX - r.left) * (W / r.width) - L) / (W - L - R)) * (x1 - x0);
    let i = 0;
    xs.forEach((v, k) => Math.abs(v - x) < Math.abs(xs[i] - x) && (i = k));
    showTip(e, series.map((s) => `${s.name} ${fmtV(s.values[i])}`).join(" · "), fmtX(xs[i]));
  });
  hit.addEventListener("pointerleave", hideTip);
  svg.append(hit);
  host.append(svg);
}

function niceStep(max: number) {
  const raw = Math.max(max, 1) / 4;
  const p = 10 ** Math.floor(Math.log10(raw));
  return [1, 2, 5, 10].map((m) => m * p).find((s) => s >= raw)!;
}

function tile(label: string, value: string, note = "") {
  const t = document.createElement("div");
  t.className = "tile";
  t.append(
    Object.assign(document.createElement("div"), { className: "label", textContent: label }),
    Object.assign(document.createElement("div"), { className: "value", textContent: value }),
  );
  if (note) t.append(Object.assign(document.createElement("div"), { className: "note", textContent: note }));
  return t;
}

function table(caption: string, head: string[], rows: (string | number)[][]) {
  const t = document.createElement("table");
  t.createCaption().textContent = caption;
  const tr = t.createTHead().insertRow();
  for (const h of head) tr.append(Object.assign(document.createElement("th"), { textContent: h }));
  const body = t.createTBody();
  for (const r of rows) {
    const row = body.insertRow();
    for (const c of r) {
      const td = row.insertCell();
      td.textContent = typeof c === "number" ? fmt.format(c) : c;
      if (typeof c === "number") td.className = "num";
    }
  }
  return t;
}

/** Curved line a→b, bulging to the right so both directions stay apart. */
function arc(a: [number, number], b: [number, number]): [number, number][] {
  const k = Math.cos((a[1] * Math.PI) / 180);
  const dx = (b[0] - a[0]) * k, dy = b[1] - a[1];
  const c: [number, number] = [(a[0] + b[0]) / 2 + (dy * 0.18) / k, (a[1] + b[1]) / 2 - dx * 0.18];
  return Array.from({ length: 17 }, (_, i) => {
    const t = i / 16, u = 1 - t;
    return [u * u * a[0] + 2 * u * t * c[0] + t * t * b[0], u * u * a[1] + 2 * u * t * c[1] + t * t * b[1]];
  });
}

/** Small arrow head for flow lines. */
function arrowImage() {
  const n = 24, c = document.createElement("canvas");
  c.width = c.height = n;
  const g = c.getContext("2d")!;
  g.fillStyle = "#fff";
  g.beginPath();
  g.moveTo(4, 4);
  g.lineTo(20, 12);
  g.lineTo(4, 20);
  g.lineTo(9, 12);
  g.closePath();
  g.fill();
  return g.getImageData(0, 0, n, n);
}

function quantileBreaks(vals: number[]) {
  const v = [...vals].sort((a, b) => a - b);
  return { max: v[v.length - 1] ?? 0, breaks: [0.2, 0.4, 0.6, 0.8].map((q) => v[Math.floor(q * (v.length - 1))] ?? 0) };
}

/** Per instance, from `/api/version` (the city comes via `loadCity`). */
/** Demo day label: a key (e.g. `christmas-saturday`) from the region; unknown values as sent. */
let demoLabel = "christmas-saturday";
const demoName = () => tOr(`dash.demo.${demoLabel}`, demoLabel);
let serverVersion = "";
/** Redraws the loaded summary in the current language (set once the map is up). */
let rerender = () => {};

/** Texts outside `data-i18n` that depend on `/api/version`. */
function applyLabels() {
  document.title = t("dash.doc.title");
  const opt = document.querySelector<HTMLOptionElement>('#data option[value="demo"]');
  if (opt) opt.textContent = t("dash.data.demo", { label: demoName() });
  if (serverVersion) $("server-version").textContent = t("dash.server", { version: serverVersion });
}

/** A login through the proxy (`opsauth.ts`), when no token is typed in. */
let login: Access | null = null;
const authed = (token: string) => !!token || !!login;

/** Regions a token or login opens (`/api/access`); null = all, or none (demo data is public). */
async function allowedRegions(token: string): Promise<string[] | null> {
  if (!authed(token)) return null;
  const { a } = await getAccess(token);
  return !a || a.all || !a.regions.length ? null : a.regions;
}

/** Next to the token field: what it opens (or that it is unknown); a login
 *  replaces the field, and "Abmelden" goes to its logout. */
async function showAccess(token: string) {
  const el = $("token-state");
  const { a, status } = await getAccess(token);
  login = !token && a?.user ? a : null;
  $("token").hidden = !!login;
  $("token-clear").hidden = login ? !login.logout : !token;
  void loadFeedback(a?.all ? token : null);
  if (login) {
    const user = login.user!;
    if (login.all) return void (el.textContent = t("dash.login.all", { user }));
    if (!login.regions.length) return void (el.textContent = t("dash.login.none", { user }));
    const names = new Map((await listRegions().catch(() => [])).map((r) => [r.slug, r.city]));
    return void (el.textContent = t("dash.login.some", { user, cities: login.regions.map((r) => names.get(r) ?? r).join(", ") }));
  }
  if (!token) return void (el.textContent = t("dash.token.none"));
  if (!a) return void (el.textContent = status === 401 ? t("dash.token.bad") : t("dash.load.failed"));
  if (a.all) return void (el.textContent = t("dash.token.all"));
  const names = new Map((await listRegions().catch(() => [])).map((r) => [r.slug, r.city]));
  el.textContent = t("dash.token.some", { cities: a.regions.map((r) => names.get(r) ?? r).join(", ") });
}

// Operator only: shown for a token or login that opens all cities.
interface FeedbackRow {
  region: string;
  day: string;
  mood: number;
  text: string | null;
  lang: string | null;
  version: string | null;
}
const FACES = ["", "☹️", "😐", "😊"];
const MOODS: Key[] = ["feedback.sad", "feedback.neutral", "feedback.happy"];
/** null: no access to the feedback. */
let fbToken: string | null = null;
let fbScope: "region" | "all" = "region";
let fbRegion = "";
let fbRows: FeedbackRow[] | "failed" | null = null;
let fbCities = new Map<string, string>();
let fbSeq = 0;

async function loadFeedback(token: string | null) {
  const seq = ++fbSeq;
  fbToken = token;
  if (token === null) {
    fbRows = null;
    return drawFeedback();
  }
  const slug = fbScope === "region" ? region || $<HTMLSelectElement>("region").value : "";
  const [res, regions] = await Promise.all([
    fetch(`/api/feedback${slug ? `?region=${encodeURIComponent(slug)}` : ""}`, { headers: authHeaders(token) }).catch(() => null),
    listRegions().catch(() => []),
  ]);
  const rows: FeedbackRow[] | null = res?.ok ? await res.json().catch(() => null) : null;
  if (seq !== fbSeq) return;
  fbRegion = slug;
  fbCities = new Map(regions.map((r) => [r.slug, r.city]));
  fbRows = res?.status === 401 ? null : (rows ?? "failed");
  drawFeedback();
}

function drawFeedback() {
  const card = $("feedback-card");
  card.hidden = fbRows === null;
  if (fbRows === null) return;
  const caption = $("fb-caption");
  if (fbRows === "failed") {
    caption.textContent = t("dash.load.failed");
    $("fb-tiles").replaceChildren();
    $("fb-list").replaceChildren();
    return;
  }
  const rows = fbRows;
  const where = fbRegion ? (fbCities.get(fbRegion) ?? fbRegion) : t("dash.fb.scope.all");
  caption.textContent = t("dash.fb.caption", { n: fmt.format(rows.length), where });
  const texts = rows.filter((r) => r.text?.trim());
  $("fb-tiles").replaceChildren(
    ...[3, 2, 1].map((m) => {
      const n = rows.filter((r) => r.mood === m).length;
      return tile(`${FACES[m]} ${t(MOODS[m - 1])}`, rows.length ? pct.format(n / rows.length) : "–", t("dash.fb.count", { n: fmt.format(n) }));
    }),
    tile(t("dash.fb.texts"), fmt.format(texts.length), t("dash.fb.texts.note", { n: fmt.format(rows.length) })),
  );
  const list = $("fb-list");
  if (!texts.length) {
    list.replaceChildren(Object.assign(document.createElement("li"), { className: "empty", textContent: t("dash.fb.empty") }));
    return;
  }
  list.replaceChildren(
    ...texts.map((r) => {
      const li = document.createElement("li");
      const meta = [
        new Date(r.day).toLocaleDateString(locale(), { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" }),
        ...(fbRegion ? [] : [fbCities.get(r.region) ?? r.region]),
        r.lang ?? "",
        r.version ?? "",
      ].filter(Boolean);
      li.append(
        Object.assign(document.createElement("span"), { className: "face", textContent: FACES[r.mood] ?? "", title: MOODS[r.mood - 1] ? t(MOODS[r.mood - 1]) : "" }),
        Object.assign(document.createElement("p"), { textContent: r.text! }),
        Object.assign(document.createElement("small"), { textContent: meta.join(" · ") }),
      );
      return li;
    }),
  );
}

for (const b of document.querySelectorAll<HTMLButtonElement>("#fb-scope button"))
  b.addEventListener("click", () => {
    fbScope = b.dataset.scope as "region" | "all";
    for (const o of document.querySelectorAll<HTMLButtonElement>("#fb-scope button")) o.classList.toggle("on", o === b);
    void loadFeedback(fbToken);
  });

// Before/after report for one place.
interface RepPeriod {
  from: string;
  to: string;
  days: number;
  days_with_data: number;
  streets: number;
  dwell_h_per_day: number;
  share_of_city: number;
  hours: number[];
  perception: Record<string, number>;
  inflow: { zone: string; name: string | null; persons: number }[];
  outflow: { zone: string; name: string | null; persons: number }[];
  interventions: { source: string; count: number; hours: number }[];
  players_per_day: number;
  player_days: number;
  thin: boolean;
  /** Counted at the focus place itself: visitors per hour summed, per day. */
  visits_per_day?: number;
  visit_h_per_day?: number;
  visit_hours?: number[];
  /** Days whose leftovers below k were pooled into one count. */
  visit_pooled_days?: number;
}
interface Report {
  region: string;
  zone: { id: string; name: string; radius_m: number; focus: boolean };
  data: string;
  a: RepPeriod;
  b: RepPeriod;
}

/** Paired columns per hour: A (grey) left, B (blue) right, same scale. */
function pairs(host: HTMLElement, a: number[], b: number[], fmtV: (v: number) => string) {
  host.replaceChildren();
  if ([...a, ...b].every((v) => v === 0)) {
    host.append(Object.assign(document.createElement("div"), { className: "empty", textContent: t("dash.empty") }));
    return;
  }
  const W = Math.max(host.clientWidth, 280), H = 200, L = 36, B = 22, T = 18;
  const max = Math.max(...a, ...b);
  const step = niceStep(max);
  const top = Math.ceil(max / step) * step;
  const svg = el("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
  for (let v = 0; v <= top; v += step) {
    const y = H - B - ((H - B - T) * v) / top;
    svg.append(el("line", { x1: L, x2: W, y1: y, y2: y, class: v === 0 ? "base" : "grid" }));
    svg.append(el("text", { x: L - 6, y: y + 4, "text-anchor": "end" }, fmtV(v)));
  }
  const band = (W - L) / a.length;
  const bw = Math.max(2, Math.min(12, band / 2 - 2));
  a.forEach((_, i) => {
    for (const [k, v, cls, name] of [[0, a[i], "bar before", "A"], [1, b[i], "bar", "B"]] as const) {
      const h = ((H - B - T) * v) / top;
      if (h <= 0) continue;
      const x = L + i * band + band / 2 - bw - 1 + k * (bw + 2);
      const y = H - B - h, r = Math.min(3, h);
      const d = `M${x},${H - B}V${y + r}Q${x},${y} ${x + r},${y}H${x + bw - r}Q${x + bw},${y} ${x + bw},${y + r}V${H - B}Z`;
      const bar = el("path", { d, class: cls, tabindex: 0 });
      const label = `${name} · ${hourRange(i)}`;
      bar.addEventListener("pointermove", (e) => showTip(e, fmtV(v), label));
      bar.addEventListener("focus", (e) => showTip(e, fmtV(v), label));
      bar.addEventListener("pointerleave", hideTip);
      bar.addEventListener("blur", hideTip);
      svg.append(bar);
    }
    if (i % 3 === 0) svg.append(el("text", { x: L + i * band + band / 2, y: H - 6, "text-anchor": "middle" }, `${i}`));
  });
  host.append(svg);
}

const isoDay = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const dayText = (s: string) => new Date(`${s}T12:00:00Z`).toLocaleDateString(locale(), { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" });
const periodText = (p: { from: string; to: string }) => (p.from === p.to ? dayText(p.from) : `${dayText(p.from)} – ${dayText(p.to)}`);
const hours1 = (v: number) => new Intl.NumberFormat(locale(), { maximumFractionDigits: v < 10 ? 1 : 0 }).format(v);

/** Relative change B vs A as text and direction (none when A is 0). */
function change(a: number, b: number): [string, string] {
  if (a === 0) return [b === 0 ? "±0" : t("dash.rep.new"), b > 0 ? "up" : ""];
  const r = b / a - 1;
  const s = new Intl.NumberFormat(locale(), { style: "percent", maximumFractionDigits: 0, signDisplay: "exceptZero" }).format(r);
  return [s, Math.abs(r) < 0.005 ? "" : r > 0 ? "up" : "down"];
}

/** Table with a change column (class up/down). */
function repTable(caption: string, head: string[], rows: { cells: string[]; ch?: [string, string] }[]) {
  const tb = document.createElement("table");
  if (caption) tb.createCaption().textContent = caption;
  const tr = tb.createTHead().insertRow();
  for (const h of head) tr.append(Object.assign(document.createElement("th"), { textContent: h }));
  const body = tb.createTBody();
  for (const r of rows) {
    const row = body.insertRow();
    r.cells.forEach((c, i) => {
      const td = row.insertCell();
      td.textContent = c;
      if (i > 0) td.className = "num";
    });
    if (r.ch) {
      const td = row.insertCell();
      td.textContent = r.ch[0];
      td.className = "num ch";
    } else if (head.length > r.cells.length) row.insertCell();
  }
  return tb;
}

let report: Report | null = null;
let reportCity = "";

function drawReport() {
  const r = report;
  $("rep-out").hidden = !r;
  $<HTMLButtonElement>("rep-csv").disabled = $<HTMLButtonElement>("rep-print").disabled = !r;
  if (!r) return;
  const { a, b } = r;
  const dataNote = r.data === "real" ? "" : ` · ${t(r.data === "synthetic" ? "dash.data.synthetic" : "dash.data.all")}`;
  $("rep-head").textContent = t("dash.rep.head", { place: r.zone.name, r: fmt.format(Math.round(r.zone.radius_m)), n: fmt.format(a.streets) }) + dataNote;
  const thin = [a.thin ? "A" : "", b.thin ? "B" : ""].filter(Boolean);
  $("rep-thin").hidden = !thin.length;
  $("rep-thin").textContent = thin.length ? t("dash.rep.thin", { which: thin.join(", ") }) : "";
  const head = ["", `A: ${periodText(a)}`, `B: ${periodText(b)}`, t("dash.rep.change")];
  // Visitors at the place: focus places only.
  const visits = r.zone.focus || (a.visits_per_day ?? 0) + (b.visits_per_day ?? 0) > 0;
  const pp = (x: number, y: number): [string, string] => {
    const d = (y - x) * 100;
    const s = new Intl.NumberFormat(locale(), { maximumFractionDigits: 1, signDisplay: "exceptZero" }).format(d);
    return [t("dash.rep.pp", { n: s }), Math.abs(d) < 0.05 ? "" : d > 0 ? "up" : "down"];
  };
  const hot = (p: RepPeriod) => {
    const n = p.interventions.reduce((s, i) => s + i.count, 0);
    const h = p.interventions.reduce((s, i) => s + i.hours, 0);
    return n ? t("dash.rep.hot.val", { n: fmt.format(n), h: hours1(h) }) : "–";
  };
  $("rep-table").replaceChildren(
    repTable("", head, [
      { cells: [t("dash.rep.dwell"), hours1(a.dwell_h_per_day), hours1(b.dwell_h_per_day)], ch: change(a.dwell_h_per_day, b.dwell_h_per_day) },
      ...(visits
        ? [
            { cells: [t("dash.rep.visits"), hours1(a.visits_per_day ?? 0), hours1(b.visits_per_day ?? 0)], ch: change(a.visits_per_day ?? 0, b.visits_per_day ?? 0) },
            { cells: [t("dash.rep.vdwell"), hours1(a.visit_h_per_day ?? 0), hours1(b.visit_h_per_day ?? 0)], ch: change(a.visit_h_per_day ?? 0, b.visit_h_per_day ?? 0) },
          ]
        : []),
      { cells: [t("dash.rep.share"), pct.format(a.share_of_city), pct.format(b.share_of_city)], ch: pp(a.share_of_city, b.share_of_city) },
      { cells: [t("dash.rep.players"), fmt.format(Math.round(a.players_per_day)), fmt.format(Math.round(b.players_per_day))], ch: change(a.players_per_day, b.players_per_day) },
      { cells: [t("dash.rep.days"), t("dash.rep.days.val", { k: a.days_with_data, n: a.days }), t("dash.rep.days.val", { k: b.days_with_data, n: b.days })] },
      { cells: [t("dash.rep.hot"), hot(a), hot(b)] },
    ]),
  );
  const leg = $("rep-legend");
  leg.replaceChildren(
    Object.assign(document.createElement("span"), { className: "sw before" }), `A ${periodText(a)}`,
    Object.assign(document.createElement("span"), { className: "sw after", style: "margin-left:12px" }), `B ${periodText(b)}`,
  );
  pairs($("rep-hours"), a.hours, b.hours, hours1);
  $("rep-visits-wrap").hidden = !visits;
  if (visits) pairs($("rep-vhours"), a.visit_hours ?? Array(24).fill(0), b.visit_hours ?? Array(24).fill(0), hours1);

  // Perception: counts and share of all ratings at the place.
  const tot = (p: RepPeriod) => Object.values(p.perception).reduce((s, n) => s + n, 0);
  const [ta, tb] = [tot(a), tot(b)];
  const cell = (n: number, total: number) => (total ? `${fmt.format(n)} (${pct.format(n / total)})` : "–");
  const perc = repTable(
    t("dash.rep.perc"),
    [t("dash.rep.perc.cat"), "A", "B", t("dash.rep.change")],
    [
      ...CATS.map((c) => {
        const x = a.perception[c] ?? 0, y = b.perception[c] ?? 0;
        return { cells: [catLabel(c), cell(x, ta), cell(y, tb)], ch: ta && tb ? pp(x / ta, y / tb) : (["–", ""] as [string, string]) };
      }),
      { cells: [t("dash.rep.perc.all"), fmt.format(ta), fmt.format(tb)], ch: change(ta / a.days, tb / b.days) },
    ],
  );
  const flowRows = (x: RepPeriod["inflow"], y: RepPeriod["inflow"]) => {
    const m = new Map<string, { name: string; a: number; b: number }>();
    for (const [k, list] of [["a", x], ["b", y]] as const)
      for (const f of list) {
        const e = m.get(f.zone) ?? { name: f.name ?? f.zone, a: 0, b: 0 };
        e[k] += f.persons;
        m.set(f.zone, e);
      }
    const rows = [...m.values()].sort((p, q) => q.a + q.b - (p.a + p.b));
    return rows.length
      ? rows.map((e) => ({ cells: [e.name, fmt.format(e.a), fmt.format(e.b)], ch: change(e.a / a.days, e.b / b.days) }))
      : [{ cells: [t("dash.rep.flows.none"), "–", "–"] }];
  };
  const fhead = [t("dash.rep.flows.zone"), "A", "B", t("dash.rep.change.day")];
  $("rep-more").replaceChildren(
    perc,
    repTable(t("dash.rep.in"), fhead, flowRows(a.inflow, b.inflow)),
    repTable(t("dash.rep.out"), fhead, flowRows(a.outflow, b.outflow)),
  );
}

function reportCsv(r: Report): string {
  const q = (v: string | number) => (typeof v === "number" ? String(Math.round(v * 10000) / 10000) : `"${v.replace(/"/g, '""')}"`);
  const rows: (string | number)[][] = [
    ["section", "item", "A", "B"],
    ["meta", "region", r.region, r.region],
    ["meta", "zone", `${r.zone.id} ${r.zone.name}`, `${r.zone.id} ${r.zone.name}`],
    ["meta", "radius_m", r.zone.radius_m, r.zone.radius_m],
    ["meta", "data", r.data, r.data],
    ["meta", "from", r.a.from, r.b.from],
    ["meta", "to", r.a.to, r.b.to],
    ["meta", "days", r.a.days, r.b.days],
    ["meta", "days_with_data", r.a.days_with_data, r.b.days_with_data],
    ["meta", "thin", String(r.a.thin), String(r.b.thin)],
    ["place", "dwell_person_hours_per_day", r.a.dwell_h_per_day, r.b.dwell_h_per_day],
    ["place", "share_of_city", r.a.share_of_city, r.b.share_of_city],
    ["city", "players_per_day", r.a.players_per_day, r.b.players_per_day],
    ["place", "visits_per_day", r.a.visits_per_day ?? 0, r.b.visits_per_day ?? 0],
    ["place", "visit_hours_per_day", r.a.visit_h_per_day ?? 0, r.b.visit_h_per_day ?? 0],
    ["place", "visit_pooled_days", r.a.visit_pooled_days ?? 0, r.b.visit_pooled_days ?? 0],
    ...Array.from({ length: 24 }, (_, h) => ["hour_visits_per_day", String(h), r.a.visit_hours?.[h] ?? 0, r.b.visit_hours?.[h] ?? 0]),
    ...Array.from({ length: 24 }, (_, h) => ["hour_person_hours_per_day", String(h), r.a.hours[h], r.b.hours[h]]),
    ...CATS.map((c) => ["perception_count", c, r.a.perception[c] ?? 0, r.b.perception[c] ?? 0]),
  ];
  for (const [sec, key] of [["inflow_persons", "inflow"], ["outflow_persons", "outflow"]] as const) {
    const ids = new Set([...r.a[key], ...r.b[key]].map((f) => f.zone));
    for (const id of ids) rows.push([sec, id, r.a[key].find((f) => f.zone === id)?.persons ?? 0, r.b[key].find((f) => f.zone === id)?.persons ?? 0]);
  }
  for (const src of new Set([...r.a.interventions, ...r.b.interventions].map((i) => i.source))) {
    const f = (p: RepPeriod, k: "count" | "hours") => p.interventions.find((i) => i.source === src)?.[k] ?? 0;
    rows.push(["hotspots_count", src, f(r.a, "count"), f(r.b, "count")], ["hotspots_hours", src, f(r.a, "hours"), f(r.b, "hours")]);
  }
  return rows.map((r) => r.map(q).join(",")).join("\n") + "\n";
}

/** The card: place picker (focus places first), periods, run, CSV, print. */
function setupReport(token: () => string, data: () => string) {
  const today = new Date();
  const back = (n: number) => isoDay(new Date(today.getFullYear(), today.getMonth(), today.getDate() - n));
  const set = (id: string, v: string) => ($<HTMLInputElement>(id).value ||= v);
  set("rep-b0", back(6));
  set("rep-b1", back(0));
  set("rep-a0", back(13));
  set("rep-a1", back(7));
  const state = $("rep-state");
  let seq = 0;
  $("rep-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const tok = token();
    if (!authed(tok)) return void (state.textContent = t("dash.rep.token"));
    const v = (id: string) => $<HTMLInputElement>(id).value;
    const d = data();
    const q = new URLSearchParams({ zone: $<HTMLSelectElement>("rep-zone").value, a: `${v("rep-a0")}..${v("rep-a1")}`, b: `${v("rep-b0")}..${v("rep-b1")}` });
    if (d !== "real") q.set("data", d === "demo" ? "synthetic" : d);
    if (region) q.set("region", region);
    const n = ++seq;
    state.textContent = t("dash.rep.loading");
    const res = await fetch(`/api/report?${q}`, { headers: authHeaders(tok) }).catch(() => null);
    const body = res?.ok ? await res.json().catch(() => null) : res ? await res.json().catch(() => null) : null;
    if (n !== seq) return;
    if (!res?.ok || !body) {
      report = null;
      state.textContent = res?.status === 401 ? t("dash.rep.token") : body?.error ? `${t("dash.load.failed")} ${body.error}` : t("dash.load.failed");
    } else {
      report = body as Report;
      state.textContent = "";
    }
    drawReport();
  });
  $("rep-csv").addEventListener("click", () => {
    if (!report) return;
    const a = Object.assign(document.createElement("a"), {
      href: URL.createObjectURL(new Blob([reportCsv(report)], { type: "text/csv" })),
      download: `elements-vorher-nachher-${report.region}-${report.zone.id}-${report.b.from}.csv`,
    });
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  });
  $("rep-print").addEventListener("click", () => {
    if (!report) return;
    const h2 = document.querySelector<HTMLElement>("#report-card > h2")!;
    h2.dataset.printMeta = t("dash.rep.printed", { city: reportCity, date: dayText(isoDay(new Date())) });
    document.body.classList.add("print-report");
    addEventListener("afterprint", () => document.body.classList.remove("print-report"), { once: true });
    print();
  });
}

/** Places for the picker: focus places first, then the other zones; keeps the choice. */
function reportZones(zones: Summary["zones"], city: string) {
  reportCity = city;
  const sel = $<HTMLSelectElement>("rep-zone");
  const keep = sel.value;
  const byName = (x: { name: string }, y: { name: string }) => x.name.localeCompare(y.name, locale());
  const group = (label: string, list: Summary["zones"]) => {
    const g = document.createElement("optgroup");
    g.label = label;
    g.append(...list.sort(byName).map((z) => new Option(z.name, z.id)));
    return g;
  };
  const focus = zones.filter((z) => z.focus), rest = zones.filter((z) => !z.focus);
  sel.replaceChildren(...(focus.length ? [group(t("dash.rep.focus"), focus)] : []), ...(rest.length ? [group(t("dash.rep.zones"), rest)] : []));
  if (keep && zones.some((z) => z.id === keep)) sel.value = keep;
}

/** A token typed in stays in this browser until "Abmelden" (localStorage); one
 * that came with a link only in its tab (sessionStorage, which wins). */
const TOKEN_KEY = "elements.ops";
function storedToken(): string {
  try {
    return sessionStorage.getItem(TOKEN_KEY) ?? localStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}
function storeToken(token: string, remember = true) {
  try {
    if (!token) {
      localStorage.removeItem(TOKEN_KEY);
      sessionStorage.removeItem(TOKEN_KEY);
    } else if (remember) {
      localStorage.setItem(TOKEN_KEY, token);
      sessionStorage.removeItem(TOKEN_KEY);
    } else {
      sessionStorage.setItem(TOKEN_KEY, token);
    }
  } catch {
    // storage blocked: this page only
  }
}

/** City picker: this server's active regions (those the token opens); a choice reloads with `?region=`.
 *  Regions of another shard (`url` set) keep their data there. */
async function regionPicker(token = "") {
  const [listed, allowed] = await Promise.all([listRegions().catch(() => []), allowedRegions(token)]);
  const all = listed.filter((r) => !("url" in r));
  const list = allowed ? all.filter((r) => allowed.includes(r.slug)) : all;
  if (allowed && list.length && !list.some((r) => r.slug === region)) {
    setRegion(list[0].slug);
    const u = new URL(location.href);
    u.searchParams.set("region", list[0].slug);
    location.replace(u.toString());
    return;
  }
  const sel = $<HTMLSelectElement>("region");
  sel.closest("label")!.hidden = list.length < 2;
  sel.replaceChildren(...list.map((r) => new Option(r.city, r.slug)));
  sel.value = region || list[0]?.slug || "";
  sel.onchange = () => {
    setRegion(sel.value);
    const u = new URL(location.href);
    u.searchParams.set("region", sel.value);
    location.href = u.toString();
  };
}

async function main() {
  fetch(withRegion("/api/version"))
    .then((r) => r.json())
    .then((v) => {
      serverVersion = String(v.version);
      demoLabel = v.demo_label ?? demoLabel;
      applyLabels();
    })
    .catch(() => {});
  const tokenInput = $<HTMLInputElement>("token");
  // From a link once, then out of the address bar (history, bookmarks,
  // screenshots) and kept for this tab only; a token typed in is remembered.
  const linked = new URLSearchParams(location.search).get("token");
  if (linked !== null) {
    const u = new URL(location.href);
    u.searchParams.delete("token");
    history.replaceState(history.state, "", u);
  }
  let remember = linked === null;
  tokenInput.value = linked ?? storedToken();
  // First: is someone logged in through the proxy (the picker and data need it).
  await showAccess(tokenInput.value.trim());
  void regionPicker(tokenInput.value.trim());
  tokenInput.addEventListener("change", () => {
    remember = true;
    void regionPicker(tokenInput.value.trim());
    void showAccess(tokenInput.value.trim());
  });
  $("token-clear").addEventListener("click", () => {
    if (!tokenInput.value.trim() && login?.logout) return void location.assign(login.logout);
    tokenInput.value = "";
    storeToken("");
    void regionPicker("");
    void showAccess("");
    $<HTMLSelectElement>("data").value = "demo";
    $("data").dispatchEvent(new Event("change", { bubbles: true }));
  });
  // Opens on the public demo day unless this browser chose another source
  // before (a remembered or autofilled token alone doesn't switch it).
  const chosen = loadKey(KEYS.dashboardData);
  const dataSel = $<HTMLSelectElement>("data");
  // Without a token or login only the demo can load: start there.
  const known = chosen && [...dataSel.options].some((o) => o.value === chosen);
  dataSel.value = known && authed(tokenInput.value.trim()) ? chosen! : "demo";
  dataSel.addEventListener("change", () => {
    saveKey(KEYS.dashboardData, dataSel.value);
  });

  // A city nobody plays in is asleep: wait until it is loaded (map first, data after).
  const meta: Meta = await loadMeta(() => {
    $("notice").textContent = t("region.waking");
    $("notice").hidden = false;
  });
  // Places come with the summary; the streets load by tiles (a city is too big at once).
  const nodePos = { get: (n: number): [number, number] | undefined => s?.places?.[n] };
  const map = new maplibregl.Map({ container: "map", style: await basemapStyle("grayscale"), bounds: meta.region.bounds, fitBoundsOptions: { padding: 10 } });
  await new Promise<void>((r) => map.on("load", () => r()));
  localLabels(map);
  map.addImage("flow-arrow", arrowImage(), { sdf: true });
  const empty = { type: "FeatureCollection" as const, features: [] };

  map.addSource("edges", { type: "geojson", data: empty });
  map.addLayer({
    id: "edges",
    type: "line",
    source: "edges",
    layout: { "line-cap": "round" },
    paint: {
      "line-color": ["coalesce", ["feature-state", "color"], "#c3c2b7"],
      "line-width": ["case", ["to-boolean", ["feature-state", "color"]], 4, 1],
    },
  });
  // Desktop: more streets at once than on a phone.
  const tiles = new TileLayer(map, meta, () => {}, () => {}, () => {}, { view: 80_000, keep: 120_000, nodes: false });
  map.on("moveend", () => tiles.refresh());
  tiles.refresh();
  map.addSource("hexes", { type: "geojson", data: empty });
  map.addLayer({
    id: "hexes",
    type: "fill",
    source: "hexes",
    paint: { "fill-color": ["get", "c"], "fill-opacity": 0.7, "fill-outline-color": "#ffffff" },
  });
  // The region's boundary and its zones (focus places gold), on top of every mode.
  map.addSource("outline", { type: "geojson", data: empty });
  map.addLayer({ id: "outline", type: "line", source: "outline", paint: { "line-color": "#495057", "line-width": 2, "line-dasharray": [3, 2] } });
  map.addSource("zone-circles", { type: "geojson", data: empty });
  map.addLayer({
    id: "zone-circles",
    type: "line",
    source: "zone-circles",
    paint: { "line-color": ["case", ["get", "focus"], "#e8a400", "#5c7cfa"], "line-width": ["case", ["get", "focus"], 2.5, 1.5], "line-opacity": 0.9 },
  });
  map.addLayer({
    id: "zone-circle-labels",
    type: "symbol",
    source: "zone-circles",
    minzoom: 13,
    layout: { "symbol-placement": "point", "text-field": ["get", "name"], "text-font": ["Noto Sans Medium"], "text-size": 12 },
    paint: { "text-color": "#343a40", "text-halo-color": "#fff", "text-halo-width": 1.5 },
  });
  void listRegions()
    .then((list) => {
      const r = list.find((x) => x.slug === meta.region.slug);
      if (r?.outline) (map.getSource("outline") as maplibregl.GeoJSONSource).setData({ type: "Feature", properties: {}, geometry: r.outline });
    })
    .catch(() => {});
  const placesBox = $<HTMLInputElement>("show-places");
  const showPlaces = () => {
    for (const l of ["outline", "zone-circles"]) map.setLayoutProperty(l, "visibility", placesBox.checked ? "visible" : "none");
    // "Wege" labels its zones already.
    map.setLayoutProperty("zone-circle-labels", "visibility", placesBox.checked && mode !== "flows" ? "visible" : "none");
  };
  placesBox.addEventListener("change", (e) => {
    e.stopPropagation();
    showPlaces();
  });
  map.addSource("areas", { type: "geojson", data: empty });
  map.addLayer({
    id: "areas",
    type: "fill",
    source: "areas",
    layout: { visibility: "none" },
    paint: { "fill-color": ["get", "c"], "fill-opacity": ["get", "o"], "fill-outline-color": "#ffffff" },
  });
  map.addSource("ratings", { type: "geojson", data: empty });
  map.addLayer({
    id: "ratings",
    type: "circle",
    source: "ratings",
    paint: {
      "circle-color": ["get", "c"],
      "circle-opacity": 0.85,
      "circle-radius": ["get", "r"],
      "circle-stroke-color": "#fff",
      "circle-stroke-width": 2,
    },
  });
  map.addSource("flows", { type: "geojson", data: empty });
  map.addLayer({
    id: "flows",
    type: "line",
    source: "flows",
    layout: { "line-cap": "round" },
    paint: { "line-color": FLOW_COLOR, "line-opacity": 0.8, "line-width": ["get", "w"] },
  });
  map.addLayer({
    id: "flow-arrows",
    type: "symbol",
    source: "flows",
    layout: {
      "symbol-placement": "line-center",
      "icon-image": "flow-arrow",
      "icon-size": ["interpolate", ["linear"], ["get", "w"], 2, 0.5, 14, 1],
      "icon-rotation-alignment": "map",
      "icon-allow-overlap": true,
    },
    paint: { "icon-color": "#fff" },
  });
  map.addSource("zones", { type: "geojson", data: empty });
  map.addLayer({
    id: "zones",
    type: "circle",
    source: "zones",
    paint: { "circle-color": "#0b0b0b", "circle-radius": 5, "circle-stroke-color": "#fff", "circle-stroke-width": 2 },
  });
  map.addLayer({
    id: "zone-labels",
    type: "symbol",
    source: "zones",
    layout: { "text-field": ["get", "name"], "text-font": ["Noto Sans Medium"], "text-size": 13, "text-variable-anchor": ["top", "bottom", "left", "right"], "text-radial-offset": 0.8 },
    paint: { "text-color": "#0b0b0b", "text-halo-color": "#fff", "text-halo-width": 2 },
  });

  let s: Summary | undefined;
  let ar: Areas = NO_AREAS;
  // Waben first: at realistic participation streets rarely reach k.
  let mode: Mode = "hexes";
  let hour = -1;
  let cat = "voll";
  let hexRes = "10";
  let timer: number | undefined;
  const shown = new Set<number>();
  const edgeVal = new Map<number, [number, number, number]>();

  // Hover popups per layer.
  const popup = new maplibregl.Popup({ closeButton: false, closeOnClick: false });
  const pop = (e: maplibregl.MapLayerMouseEvent, lines: string[]) => {
    const div = document.createElement("div");
    div.append(Object.assign(document.createElement("strong"), { textContent: lines[0] }));
    for (const l of lines.slice(1)) div.append(document.createElement("br"), l);
    popup.setLngLat(e.lngLat).setDOMContent(div).addTo(map);
  };
  map.on("mousemove", "edges", (e) => {
    if (mode !== "dwell") return;
    const d = edgeVal.get(e.features?.[0]?.id as number);
    if (!d) return void popup.remove();
    pop(e, [t("dash.pq", { n: fmt.format(d[0]), count: d[0] }), d[1] >= 0 ? dwellText(d[1], d[2]) : hourText(hour)]);
  });
  map.on("mousemove", "hexes", (e) => {
    const p = e.features?.[0]?.properties;
    if (p) pop(e, [t("dash.ph", { n: fmt.format(p.v), count: p.v }), hour < 0 ? dwellText(p.d, p.h) : hourText(hour)]);
  });
  map.on("mousemove", "ratings", (e) => {
    const node = e.features?.[0]?.properties?.node as number | undefined;
    const r = node !== undefined ? placeOf.get(node) : undefined;
    if (!r) return void popup.remove();
    const mix = [...r.by].sort((a, b) => b[1] - a[1]).map(([c, n]) => `${catLabel(c)} ${pct.format(n / r.total)}`);
    pop(e, [
      t("dash.map.ratings.popup", { pct: pct.format((r.by.get(cat) ?? 0) / r.total), cat: catLabel(cat), k: fmt.format(r.by.get(cat) ?? 0), n: fmt.format(r.total) }),
      mix.join(" · "),
      ...(r.zone ? [t("dash.near", { zone: r.zone })] : []),
    ]);
  });
  map.on("mousemove", "flows", (e) => {
    const p = e.features?.[0]?.properties;
    if (p) pop(e, [t("dash.persons", { n: fmt.format(p.n), count: p.n }), `${p.from} → ${p.to}`]);
  });
  map.on("mousemove", "areas", (e) => {
    const p = e.features?.[0]?.properties;
    if (!p) return;
    const h = (m: number) => hours1(m / 60);
    pop(e, [t("dash.area.popup", { f: h(p.f), w: h(p.w) }), t("dash.area.popup.ha", { ha: hours1(p.ha) })]);
  });
  for (const l of ["edges", "hexes", "ratings", "flows", "areas"]) map.on("mouseleave", l, () => popup.remove());

  const hourText = (h: number) => (h < 0 ? t("dash.hour.all") : hourRange(h));

  function legendRamp(max: number, what: string) {
    const legend = $("map-legend");
    legend.replaceChildren(t("dash.legend.low"));
    for (const c of RAMP) legend.append(Object.assign(document.createElement("span"), { className: "sw", style: `background:${c}` }));
    legend.append(t("dash.legend.high", { max: fmt.format(max) + (what ? ` ${what}` : "") }));
    const grey = Object.assign(document.createElement("span"), { className: "sw", style: "background:#c3c2b7;margin-left:12px" });
    legend.append(grey, t("dash.legend.below", { k: s!.k }));
  }

  function drawDwell() {
    for (const id of shown) map.removeFeatureState({ source: "edges", id });
    shown.clear();
    edgeVal.clear();
    if (!s) return;
    // Whole day: totals. One hour: same scale for all hours so the replay compares.
    const rows: [number, number, number, number][] = hour < 0 ? s.dwell : s.hourly_edges.filter((r) => r[1] === hour).map(([e, , p]) => [e, p, -1, 0]);
    const { breaks, max } = quantileBreaks(hour < 0 ? s.dwell.map((d) => d[1]) : s.hourly_edges.map((d) => d[2]));
    for (const [edge, persons, dwell, hot] of rows) {
      edgeVal.set(edge, [persons, dwell, hot]);
      shown.add(edge);
      map.setFeatureState({ source: "edges", id: edge }, { color: RAMP[breaks.filter((b) => persons > b).length] });
    }
    $("hour-label").textContent = hourText(hour);
    $<HTMLInputElement>("hour").value = String(hour);
    $("map-title").textContent = t("dash.map.dwell.title");
    $("map-caption").textContent =
      (hour < 0 ? t("dash.map.dwell.day") : t("dash.map.dwell.hour", { h: hour, h2: hour + 1 })) + " " + t("dash.map.dwell.rule", { k: s.k });
    if (max) legendRamp(max, "");
    else $("map-legend").replaceChildren(t("dash.empty"));
  }

  function drawHexes() {
    if (!s) return;
    const h = s.hexes?.[hexRes];
    const cells = h?.cells.features ?? [];
    // Whole day: totals. One hour: same scale for all hours so the replay compares.
    const perHour = new Map<number, number>();
    if (hour >= 0) for (const [i, hr, p] of h?.hourly ?? []) if (hr === hour) perHour.set(i, p);
    const { breaks, max } = quantileBreaks(hour < 0 ? cells.map((f) => f.properties.persons) : (h?.hourly ?? []).map((r) => r[2]));
    const features = cells.flatMap((f, i) => {
      const v = hour < 0 ? f.properties.persons : perHour.get(i);
      if (!v) return [];
      const c = RAMP[breaks.filter((b) => v > b).length];
      return [{ ...f, properties: { v, d: f.properties.dwell_s, h: f.properties.hotspot_dwell_s ?? 0, c } }];
    });
    (map.getSource("hexes") as maplibregl.GeoJSONSource).setData({ type: "FeatureCollection", features });
    $("hour-label").textContent = hourText(hour);
    $<HTMLInputElement>("hour").value = String(hour);
    $("map-title").textContent = t("dash.map.hexes.title");
    const share = h && h.kept + h.dropped > 0 ? pct.format(h.kept / (h.kept + h.dropped)) : "–";
    const size = hexSize(hexRes);
    $("map-caption").textContent =
      (hour < 0 ? t("dash.map.hexes.day", { size }) : t("dash.map.hexes.hour", { h: hour, h2: hour + 1, size })) +
      " " +
      t("dash.map.hexes.rule", { k: s.k, share });
    if (max) legendRamp(max, "");
    else $("map-legend").replaceChildren(t("dash.empty"));
  }

  // Ratings per place: total and per category, plus the nearest zone as a name.
  type Place = { node: number; total: number; by: Map<string, number>; zone: string | null };
  // Map: places of the chosen hour (or the whole day); list and table: whole day.
  let placeOf = new Map<number, Place>();
  let placesDay = new Map<number, Place>();
  function places(rows: [number, string, number][]) {
    const out = new Map<number, Place>();
    for (const [node, c, n] of rows) {
      let p = out.get(node);
      if (!p) {
        const pos = nodePos.get(node);
        p = { node, total: 0, by: new Map(), zone: pos ? nearestZone(pos) : null };
        out.set(node, p);
      }
      p.total += n;
      p.by.set(c, (p.by.get(c) ?? 0) + n);
    }
    return out;
  }
  function nearestZone([lon, lat]: [number, number]) {
    let best: [string, number] | null = null;
    for (const z of s?.zones ?? []) {
      const d = distM([lon, lat], [z.lon, z.lat]);
      if (d <= 400 && (!best || d < best[1])) best = [z.name, d];
    }
    return best?.[0] ?? null;
  }
  const shareColor = (share: number) => RAMP[SHARE_BREAKS.filter((b) => share > b).length];

  function drawRatings() {
    if (!s) return;
    placeOf = hour < 0 ? placesDay : places(s.perception_hourly.filter((r) => r[2] === hour).map(([n, c, , k]) => [n, c, k]));
    const max = Math.max(1, ...[...placeOf.values()].map((p) => p.total));
    const features = [...placeOf.values()].flatMap((p) => {
      const pos = nodePos.get(p.node);
      if (!pos) return [];
      const share = (p.by.get(cat) ?? 0) / p.total;
      const c = shareColor(share);
      return [{ type: "Feature" as const, properties: { node: p.node, c, r: 4 + 12 * Math.sqrt(p.total / max) }, geometry: { type: "Point" as const, coordinates: pos } }];
    });
    (map.getSource("ratings") as maplibregl.GeoJSONSource).setData({ type: "FeatureCollection", features });
    $("map-title").textContent = t("dash.map.ratings.title");
    $("hour-label").textContent = hourText(hour);
    $<HTMLInputElement>("hour").value = String(hour);
    const vars = { cat: catLabel(cat), min: s.min_ratings, n: fmt.format(placeOf.size), h: hour, h2: hour + 1 };
    $("map-caption").textContent = hour < 0 ? t("dash.map.ratings.day", vars) : t("dash.map.ratings.hour", vars);
    const legend = $("map-legend");
    legend.replaceChildren(pct.format(0));
    for (const c of RAMP) legend.append(Object.assign(document.createElement("span"), { className: "sw", style: `background:${c}` }));
    legend.append(pct.format(1));
    drawPlaces();
    drawRatingHours();
  }

  // Share of the chosen impression per local hour, over all places.
  function drawRatingHours() {
    if (!s) return;
    const label = catLabel(cat);
    const all = new Map<number, number>(), hit = new Map<number, number>();
    for (const [h, c, n] of s.perception_by_hour) {
      all.set(h, (all.get(h) ?? 0) + n);
      if (c === cat) hit.set(h, (hit.get(h) ?? 0) + n);
    }
    $("rating-hours-title").textContent = t("dash.ratinghours.title", { cat: label });
    $("rating-hours-caption").textContent = t("dash.ratinghours.caption", { cat: label, min: s.min_ratings });
    columns(
      $("rating-hours"),
      Array.from({ length: 24 }, (_, h) => {
        const n = all.get(h) ?? 0, k = hit.get(h) ?? 0;
        return {
          label: `${h}`,
          value: n ? k / n : 0,
          tip: n ? t("dash.share.of", { pct: pct.format(k / n), k: fmt.format(k), n: fmt.format(n) }) : t("dash.ratinghours.few"),
          tipLabel: hourRange(h),
        };
      }),
      { every: 3, fmtValue: (v) => pct.format(v) },
    );
  }

  // Places where the chosen impression is clearest; click flies there.
  function drawPlaces() {
    const label = catLabel(cat);
    $("places-title").textContent = t("dash.places.title", { cat: label });
    $("places-caption").textContent = t("dash.places.caption", { cat: label, min: MIN_RATINGS_LIST });
    const top = [...placesDay.values()]
      .filter((p) => p.total >= MIN_RATINGS_LIST && (p.by.get(cat) ?? 0) > 0)
      .map((p) => ({ p, share: (p.by.get(cat) ?? 0) / p.total }))
      .sort((a, b) => b.share - a.share || b.p.total - a.p.total)
      .slice(0, 6);
    const host = $("places");
    hbars(
      host,
      top.map(({ p, share }, i) => ({
        label: `${i + 1}. ${p.zone ?? t("dash.node", { node: p.node })}`,
        value: share,
        valueText: `${pct.format(share)} (${fmt.format(p.total)})`,
        tip: t("dash.places.tip", { k: fmt.format(p.by.get(cat) ?? 0), n: fmt.format(p.total) }),
      })),
    );
    host.querySelectorAll<SVGPathElement>("path.bar").forEach((bar, i) => {
      bar.style.cursor = "pointer";
      bar.addEventListener("click", () => {
        if (mode !== "ratings") setMode("ratings");
        const pos = nodePos.get(top[i].p.node);
        if (pos) map.flyTo({ center: pos, zoom: 17.5 });
        document.getElementById("map")?.scrollIntoView({ behavior: "smooth", block: "center" });
      });
    });
  }

  function drawFlows() {
    if (!s) return;
    const zone = new Map(s.zones.map((z) => [z.id, z]));
    (map.getSource("zones") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: s.zones.map((z) => ({ type: "Feature", properties: { name: z.name }, geometry: { type: "Point", coordinates: [z.lon, z.lat] } })),
    });
    const max = Math.max(1, ...s.flows.map((f) => f[2]));
    const features = s.flows.flatMap(([a, b, n]) => {
      const za = zone.get(a), zb = zone.get(b);
      if (!za || !zb) return [];
      return [{
        type: "Feature" as const,
        properties: { n, w: 2 + 12 * (n / max), from: za.name, to: zb.name },
        geometry: { type: "LineString" as const, coordinates: arc([za.lon, za.lat], [zb.lon, zb.lat]) },
      }];
    });
    (map.getSource("flows") as maplibregl.GeoJSONSource).setData({ type: "FeatureCollection", features });
    const total = s.flows.reduce((a, f) => a + f[2], 0);
    $("map-title").textContent = t("dash.flows.title");
    $("map-caption").textContent = t("dash.flows.caption", { n: fmt.format(total), k: s.k });
    const legend = $("map-legend");
    legend.replaceChildren();
    for (const n of [Math.max(1, Math.round(max / 4)), max]) {
      legend.append(Object.assign(document.createElement("span"), { className: "sw", style: `background:${FLOW_COLOR};height:${2 + 12 * (n / max)}px;border-radius:2px` }), `${t("dash.persons", { n: fmt.format(n), count: n })} `);
    }
    if (!features.length) legend.replaceChildren(t("dash.flows.empty"));
  }

  // Blocks by the side that held them longer; fainter = held for less of the period.
  function drawAreas() {
    const feats = ar.blocks.features;
    const most = Math.max(1, ...feats.map((f) => f.properties.f + f.properties.w));
    (map.getSource("areas") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: feats.map((f) => {
        const { f: fm, w: wm } = f.properties;
        return { ...f, properties: { ...f.properties, c: fm >= wm ? COLORS.fire : COLORS.water, o: 0.2 + 0.65 * Math.sqrt((fm + wm) / most) } };
      }),
    });
    $("map-title").textContent = t("dash.area.map.title");
    $("map-caption").textContent = t("dash.area.map.caption", { n: fmt.format(feats.length) });
    const legend = $("map-legend");
    legend.replaceChildren();
    for (const [c, k] of [[COLORS.fire, "dash.area.fire"], [COLORS.water, "dash.area.water"]] as const)
      legend.append(Object.assign(document.createElement("span"), { className: "sw", style: `background:${c};height:10px` }), `${t(k)} `);
    legend.append(`· ${t("dash.area.legend")}`);
    if (!feats.length) legend.replaceChildren(t("dash.area.none"));
  }

  function setMode(m: Mode) {
    mode = m;
    stop();
    for (const b of document.querySelectorAll<HTMLButtonElement>(".tabs button")) {
      b.classList.toggle("on", b.dataset.mode === m);
      b.setAttribute("aria-selected", String(b.dataset.mode === m));
    }
    $("ctl-dwell").hidden = m === "flows" || m === "areas";
    map.setLayoutProperty("areas", "visibility", m === "areas" ? "visible" : "none");
    showPlaces();
    $("ctl-hex").hidden = m !== "hexes";
    map.setLayoutProperty("hexes", "visibility", m === "hexes" ? "visible" : "none");
    $("ctl-ratings").hidden = m !== "ratings";
    map.setLayoutProperty("ratings", "visibility", m === "ratings" ? "visible" : "none");
    for (const l of ["flows", "flow-arrows", "zones", "zone-labels"]) map.setLayoutProperty(l, "visibility", m === "flows" ? "visible" : "none");
    map.setPaintProperty("edges", "line-opacity", m === "dwell" ? 1 : 0.35);
    if (m !== "dwell") for (const id of shown) map.removeFeatureState({ source: "edges", id });
    redraw();
  }
  const redraw = () =>
    mode === "dwell" ? drawDwell() : mode === "hexes" ? drawHexes() : mode === "ratings" ? drawRatings() : mode === "areas" ? drawAreas() : drawFlows();

  // Replay: step through all 24 hours; empty hours show an empty map.
  function stop() {
    if (timer !== undefined) clearInterval(timer);
    timer = undefined;
    $("play").textContent = "▶";
    $("play").setAttribute("aria-label", t("dash.play"));
  }
  $("play").addEventListener("click", () => {
    if (timer !== undefined) return stop();
    if (!s?.hourly_edges.length) return;
    const step = () => {
      hour = (hour + 1) % 24;
      redraw();
    };
    step();
    timer = setInterval(step, 900);
    $("play").textContent = "❚❚";
    $("play").setAttribute("aria-label", t("dash.pause"));
  });
  $("hour").addEventListener("input", () => {
    stop();
    hour = Number($<HTMLInputElement>("hour").value);
    redraw();
  });
  for (const b of document.querySelectorAll<HTMLButtonElement>(".tabs button")) b.addEventListener("click", () => setMode(b.dataset.mode as Mode));
  for (const b of document.querySelectorAll<HTMLButtonElement>("#ctl-hex button"))
    b.addEventListener("click", () => {
      hexRes = b.dataset.res!;
      for (const o of document.querySelectorAll<HTMLButtonElement>("#ctl-hex button")) o.classList.toggle("on", o === b);
      drawHexes();
    });
  for (const b of document.querySelectorAll<HTMLButtonElement>("#ctl-ratings button"))
    b.addEventListener("click", () => {
      cat = b.dataset.cat!;
      for (const o of document.querySelectorAll<HTMLButtonElement>("#ctl-ratings button")) o.classList.toggle("on", o === b);
      drawRatings();
    });

  // Demo: the synthetic rows of the server's demo day instead of a period.
  const query = () => {
    const data = $<HTMLSelectElement>("data").value;
    const q = data === "demo" ? "day=demo&data=synthetic" : `days=${$<HTMLSelectElement>("days").value}&data=${data}`;
    return region ? `${q}&region=${encodeURIComponent(region)}` : q;
  };

  // Downloads go through fetch so the token stays in a header.
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-export]"))
    b.addEventListener("click", async () => {
      const name = b.dataset.export!;
      const res = await fetch(`/api/export/${name}?${query()}`, {
        headers: authHeaders(tokenInput.value.trim()),
      });
      if (!res.ok) {
        $("notice").textContent = res.status === 401 ? t("dash.export.denied") : t("dash.export.failed");
        $("notice").hidden = false;
        return;
      }
      const a = Object.assign(document.createElement("a"), { href: URL.createObjectURL(await res.blob()), download: `elements-${name}` });
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    });

  setupReport(() => tokenInput.value.trim(), () => $<HTMLSelectElement>("data").value);

  // Only the latest request may draw: a slow earlier answer (e.g. the demo day)
  // must not overwrite a later selection.
  let loadSeq = 0;
  /** Data source of the summary on screen. */
  let shownData = "demo";
  async function load() {
    const seq = ++loadSeq;
    const token = tokenInput.value.trim();
    if (token) storeToken(token, remember);
    const data = $<HTMLSelectElement>("data").value;
    const notice = $("notice");
    $<HTMLSelectElement>("days").disabled = data === "demo";
    document.body.style.opacity = "0.6";
    const [res, ares] = await Promise.all([
      fetch(`/api/analytics?${query()}`, { headers: authHeaders(token) }),
      fetch(`/api/areas?${query()}`, { headers: authHeaders(token) }).catch(() => null),
    ]);
    const body = res.ok ? await res.json() : null;
    const areas: Areas = ares?.ok ? await ares.json().catch(() => NO_AREAS) : NO_AREAS;
    if (seq !== loadSeq) return;
    ar = areas;
    document.body.style.opacity = "";
    if (!res.ok) {
      // Nothing from before stays on screen: it would look like the answer.
      stop();
      s = EMPTY;
      ar = NO_AREAS;
      shownData = data;
      $<HTMLButtonElement>("play").disabled = true;
      render();
      notice.textContent = res.status === 401 ? t("dash.load.denied") : t("dash.load.failed");
      notice.hidden = false;
      return;
    }
    stop();
    s = body as Summary;
    shownData = data;
    // Frame the segments that have data (once per load).
    if (s.dwell_bounds) map.fitBounds(s.dwell_bounds, { padding: 40, maxZoom: 16.5, duration: 0 });
    $<HTMLButtonElement>("play").disabled = !s.hourly_edges.length;
    render();
  }

  /** Draws the loaded summary; again after a language switch. */
  function render() {
    if (!s) return;
    const d = s;
    const data = shownData;
    const notice = $("notice");
    placesDay = places(d.perception);
    placeOf = placesDay;
    notice.hidden = false;
    const released = " " + t("dash.notice.released");
    notice.textContent =
      data === "demo"
        ? t("dash.notice.demo", {
            label: demoName(),
            date: d.day ? new Date(d.day).toLocaleDateString(locale(), { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" }) : "",
            n: fmt.format(d.daily.reduce((a, x) => a + x[1], 0)),
            k: d.k,
          })
        : data === "synthetic"
          ? t("dash.notice.synthetic")
          : data === "real"
            ? t("dash.notice.real") + released
            : t("dash.notice.all") + released;

    // Tiles
    const personQ = d.dwell.reduce((a, x) => a + x[1], 0);
    const dwellH = d.dwell.reduce((a, x) => a + x[2], 0) / 3600;
    const hotH = d.dwell.reduce((a, x) => a + (x[3] ?? 0), 0) / 3600;
    const act = d.experiment.find((e) => e.active);
    const ctl = d.experiment.find((e) => !e.active);
    const rate = (e?: { nearby: number; reached_nearby: number }) => (e && e.nearby ? e.reached_nearby / e.nearby : 0);
    $("tiles").replaceChildren(
      tile(t("dash.tile.pq"), fmt.format(personQ), t("dash.tile.pq.note", { n: fmt.format(d.dwell.length) })),
      tile(
        t("dash.tile.dwell"),
        t("dash.tile.dwell.value", { h: fmt.format(Math.round(dwellH)) }),
        dwellH > 0 ? t("dash.tile.dwell.hot", { pct: pct.format(hotH / dwellH) }) : t("dash.tile.dwell.sum"),
      ),
      tile(
        t("dash.tile.follow"),
        act ? pct.format(rate(act)) : "–",
        (ctl ? t("dash.tile.follow.ctl", { pct: pct.format(rate(ctl)) }) : "") +
          (act && act.slots < 30 ? ` · ${t("dash.tile.follow.prelim", { n: fmt.format(act.slots) })}` : ""),
      ),
    );

    redraw();
    renderAreas();
    (map.getSource("zone-circles") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: d.zones.map((z) => ({ ...circle([z.lon, z.lat], z.radius_m ?? 400), properties: { name: z.name, focus: !!z.focus } })),
    });
    reportZones(d.zones, document.querySelector(".city")?.textContent ?? "");
    drawReport();

    // Hourly
    const byHour = new Map(d.hourly);
    columns(
      $("hourly"),
      Array.from({ length: 24 }, (_, h) => ({
        label: `${h}`,
        value: byHour.get(h) ?? 0,
        tip: t("dash.pq", { n: fmt.format(byHour.get(h) ?? 0), count: byHour.get(h) ?? 0 }),
        tipLabel: hourRange(h),
      })),
      { every: 3 },
    );

    // Ratings per place (map mode and the list next to the hourly chart)
    drawPlaces();
    drawRatingHours();
    const ratings = d.perception.reduce((a, x) => a + x[2], 0);

    // Experiment
    const exp = [
      { name: t("dash.exp.with"), e: act },
      { name: t("dash.exp.without"), e: ctl },
    ].filter((x) => x.e);
    hbars(
      $("experiment"),
      exp.map(({ name, e }) => ({
        label: name,
        value: rate(e),
        valueText: pct.format(rate(e)),
        tip: t("dash.exp.tip", { k: fmt.format(e!.reached_nearby), n: fmt.format(e!.nearby), slots: fmt.format(e!.slots) }),
      })),
    );

    // Player mix: shares among those who answered; counts if a group is hidden (below k).
    const mix = (cells: [string, number | null][], label: (k: string) => string, host: string) => {
      const answers = cells.filter(([k]) => k !== "na");
      const hidden = answers.some(([, n]) => n === null);
      const answered = answers.reduce((a, [, n]) => a + (n ?? 0), 0);
      hbars(
        $(host),
        answered
          ? answers.map(([k, n]) => ({
              label: label(k),
              value: hidden ? (n ?? 0) : (n ?? 0) / answered,
              valueText: n === null ? t("dash.mix.below", { k: d.k }) : hidden ? fmt.format(n) : pct.format(n / answered),
              tip: n === null ? t("dash.mix.hidden") : t("dash.mix.pd", { n: fmt.format(n), count: n }),
            }))
          : [],
      );
      const na = cells.find(([k]) => k === "na")?.[1];
      return hidden || na === null || na === undefined ? null : answered / (answered + na);
    };
    const ageRate = mix(d.profiles.age, ageLabel, "mix-age");
    const localRate = mix(d.profiles.local, (k) => (k === "yes" ? t("dash.mix.local.yes") : t("dash.mix.local.no")), "mix-local");
    const rateText = (r: number | null) => (r === null ? "–" : pct.format(r));
    $("mix-caption").textContent =
      t("dash.mix.caption") + " " + t("dash.mix.caption.more", { k: d.k, age: rateText(ageRate), local: rateText(localRate) });

    // Growth
    const gr = d.growth ?? [];
    const last = gr.at(-1);
    const atLeastK = (n: number) => (n >= d.k ? fmt.format(n) : t("dash.gr.below", { k: d.k }));
    const hits = (d.links ?? []).reduce((a, [, n]) => a + n, 0);
    $("growth-tiles").replaceChildren(
      tile(t("dash.gr.total"), last ? atLeastK(last[4]) : "–", t("dash.gr.total.note")),
      tile(t("dash.gr.multi"), last ? atLeastK(last[3]) : "–", t("dash.gr.multi.note")),
      tile(t("dash.gr.new"), gr.length ? atLeastK(gr.reduce((a, x) => a + x[2], 0)) : "–", t("dash.gr.new.note")),
      tile(
        t("dash.gr.peak"),
        d.peak ? fmt.format(d.peak[0]) : "–",
        d.peak
          ? t("dash.gr.peak.note", {
              when: new Date(d.peak[1] * 1000).toLocaleString(locale(), { weekday: "short", day: "numeric", month: "numeric", hour: "2-digit", minute: "2-digit" }),
            })
          : t("dash.gr.peak.none", { k: d.k }),
      ),
      tile(t("dash.gr.links"), fmt.format(hits), (d.links ?? []).map(([c, n]) => `/${c} ${fmt.format(n)}`).join(" · ") || t("dash.gr.links.note")),
    );
    columns(
      $("growth-chart"),
      gr.map(([day, n, fresh]) => ({
        label: new Date(day).toLocaleDateString(locale(), { day: "numeric", month: "numeric" }),
        value: n,
        tip: t("dash.gr.tip", { n: fmt.format(n), fresh: fmt.format(fresh) }),
        tipLabel: new Date(day).toLocaleDateString(locale(), { weekday: "long", day: "numeric", month: "long" }),
      })),
      { every: Math.ceil(gr.length / 10) },
    );

    // Transparency
    const win = d.suppressed.kept + d.suppressed.dropped;
    const avgPlayers = d.daily.length ? d.daily.reduce((a, x) => a + x[1], 0) / d.daily.length : 0;
    $("transparency").replaceChildren(
      tile(
        t("dash.tile.players"),
        fmt.format(Math.round(avgPlayers)),
        d.daily.length === 1 ? t("dash.tile.players.one") : t("dash.tile.players.avg", { n: fmt.format(d.daily.length) }),
      ),
      tile(t("dash.tile.k"), fmt.format(d.k), t("dash.tile.k.note")),
      tile(
        t("dash.tile.withheld"),
        win ? pct.format(d.suppressed.dropped / win) : "–",
        t("dash.tile.withheld.note", { n: fmt.format(d.suppressed.dropped), total: fmt.format(win) }),
      ),
      tile(t("dash.tile.ratings"), fmt.format(ratings), t("dash.tile.ratings.note", { n: fmt.format(placesDay.size) })),
      tile(t("dash.tile.traces"), "0", t("dash.tile.traces.note")),
      tile(
        t("dash.tile.short"),
        fmt.format(Math.round(d.daily.length ? d.daily.reduce((a, x) => a + (x[2] ?? 0), 0) / d.daily.length : 0)),
        t("dash.tile.short.note"),
      ),
      tile(
        t("dash.tile.flagged"),
        fmt.format(d.daily.reduce((a, x) => a + (x[3] ?? 0), 0)),
        t("dash.tile.flagged.note"),
      ),
      tile(t("dash.tile.noise"), "± 2", t("dash.tile.noise.note")),
    );
    // The first five minutes: new players who claim within five minutes of joining.
    const joined = d.daily.reduce((a, x) => a + (x[4] ?? 0), 0);
    const quick = d.daily.reduce((a, x) => a + (x[5] ?? 0), 0);
    $("transparency").append(
      tile(t("dash.tile.first"), joined ? pct.format(quick / joined) : "–", t("dash.tile.first.note", { n: fmt.format(quick), total: fmt.format(joined) })),
    );
    columns(
      $("daily"),
      d.daily.map(([day, n]) => ({
        label: new Date(day).toLocaleDateString(locale(), { day: "numeric", month: "numeric" }),
        value: n,
        tip: t("dash.players", { n: fmt.format(n), count: n }),
        tipLabel: new Date(day).toLocaleDateString(locale(), { weekday: "long", day: "numeric", month: "long" }),
      })),
      { every: Math.ceil(d.daily.length / 10) },
    );

    // Tables
    $("tables").replaceChildren(
      table(t("dash.t.hourly"), [t("dash.hour"), t("dash.tile.pq")], Array.from({ length: 24 }, (_, h) => [hourRange(h), byHour.get(h) ?? 0])),
      table(
        t("dash.t.places"),
        [t("dash.t.place"), t("dash.t.ratings"), ...CATS.map(catLabel)],
        [...placesDay.values()]
          .sort((a, b) => b.total - a.total)
          .map((p) => [
            p.zone ? t("dash.near.node", { zone: p.zone, node: p.node }) : t("dash.node", { node: p.node }),
            p.total,
            ...CATS.map((c) => pct.format((p.by.get(c) ?? 0) / p.total)),
          ]),
      ),
      table(t("dash.t.exp"), [t("dash.t.group"), t("dash.t.slots"), t("dash.t.nearby"), t("dash.t.reached"), t("dash.t.reached.all")], exp.map(({ name, e }) => [name, e!.slots, e!.nearby, e!.reached_nearby, e!.reached])),
      table(
        t("dash.t.ratinghours"),
        [t("dash.hour"), t("dash.t.ratings"), ...CATS.map(catLabel)],
        Array.from({ length: 24 }, (_, h) => h).flatMap((h) => {
          const rows = d.perception_by_hour.filter((r) => r[0] === h);
          const n = rows.reduce((a, r) => a + r[2], 0);
          return n ? [[hourRange(h), n, ...CATS.map((c) => pct.format(rows.filter((r) => r[1] === c).reduce((a, r) => a + r[2], 0) / n))]] : [];
        }),
      ),
      table(t("dash.flows.title"), [t("dash.t.from"), t("dash.t.to"), t("dash.t.persons")], d.flows.map(([a, b, n]) => [d.zones.find((z) => z.id === a)?.name ?? a, d.zones.find((z) => z.id === b)?.name ?? b, n])),
      table(t("dash.tile.players"), [t("dash.t.day"), t("dash.t.players")], d.daily.map(([day, n]) => [day, n])),
      table(
        t("dash.area.t"),
        [t("dash.t.day"), t("dash.area.fire"), t("dash.area.water"), t("dash.area.t.fp"), t("dash.area.t.wp")],
        areaDays().map(([day, v]) => [day, ...v.map((x) => hours1(x))]),
      ),
      table(
        t("dash.t.mix"),
        [t("dash.t.answer"), t("dash.t.count")],
        [
          ...d.profiles.age.map(([a, n]): [string, string | number] => [
            t("dash.t.age", { v: AGES.includes(a) ? ageLabel(a) : t("dash.t.none") }),
            n ?? t("dash.mix.below", { k: d.k }),
          ]),
          ...d.profiles.local.map(([l, n]): [string, string | number] => [
            l === "yes" ? t("dash.t.home.yes") : t("dash.t.home", { v: l === "no" ? t("dash.t.home.no") : t("dash.t.none") }),
            n ?? t("dash.mix.below", { k: d.k }),
          ]),
        ],
      ),
    );
  }

  /** Per local day: mean ha (fire, water, fire with players, water with players), weighted by minutes. */
  function areaDays(): [string, number[]][] {
    const by = new Map<string, { n: number; v: number[] }>();
    for (const [h, f, w, fp, wp, , , m] of ar.hours) {
      const day = new Date(h * 1000).toLocaleDateString("sv-SE");
      const e = by.get(day) ?? { n: 0, v: [0, 0, 0, 0] };
      [f, w, fp, wp].forEach((x, i) => (e.v[i] += x * m));
      e.n += m;
      by.set(day, e);
    }
    return [...by].map(([day, e]) => [day, e.v.map((x) => x / Math.max(1, e.n))]);
  }

  function renderAreas() {
    const h = ar.hours;
    const mins = h.reduce((a, r) => a + r[7], 0);
    const mean = (i: number) => (mins ? h.reduce((a, r) => a + r[i] * r[7], 0) / mins : 0);
    const peak = (i: number) => Math.max(0, ...h.map((r) => r[i]));
    const total = ar.total_ha ?? 0;
    const share = (ha: number) => (total ? ` · ${pct.format(ha / total)}` : "");
    $("areas-tiles").replaceChildren(
      tile(t("dash.area.fire"), t("dash.area.ha", { ha: hours1(mean(1)) }), t("dash.area.tile.note", { p: hours1(mean(3)), max: hours1(peak(5)) }) + share(mean(1))),
      tile(t("dash.area.water"), t("dash.area.ha", { ha: hours1(mean(2)) }), t("dash.area.tile.note", { p: hours1(mean(4)), max: hours1(peak(6)) }) + share(mean(2))),
      tile(t("dash.area.hours"), fmt.format(h.length), t("dash.area.hours.note", { total: fmt.format(Math.round(total)) })),
    );
    const legend = $("areas-legend");
    legend.replaceChildren();
    for (const [c, k, thin] of [[COLORS.fire, "dash.area.fire", false], [COLORS.water, "dash.area.water", false], [COLORS.neutral, "dash.area.players", true]] as const)
      legend.append(Object.assign(document.createElement("span"), { className: `ln${thin ? " thin" : ""}`, style: `border-color:${c}` }), `${t(k)} `);
    const xs = h.map((r) => r[0]);
    const long = xs.length > 1 && xs[xs.length - 1] - xs[0] > 2 * 86_400;
    lines(
      $("areas-chart"),
      xs,
      [
        { values: h.map((r) => r[1]), color: COLORS.fire, name: t("dash.area.fire") },
        { values: h.map((r) => r[2]), color: COLORS.water, name: t("dash.area.water") },
        { values: h.map((r) => r[3]), color: COLORS.fire, thin: true, name: `${t("dash.area.fire")} (${t("dash.area.players")})` },
        { values: h.map((r) => r[4]), color: COLORS.water, thin: true, name: `${t("dash.area.water")} (${t("dash.area.players")})` },
      ],
      (v) => `${hours1(v)} ha`,
      (x) => new Date(x * 1000).toLocaleString(locale(), long ? { day: "numeric", month: "short" } : { weekday: "short", hour: "2-digit", minute: "2-digit" }),
    );
    $<HTMLButtonElement>("areas-csv").disabled = !h.length;
  }
  $("areas-csv").addEventListener("click", () => {
    const head = "hour_start_utc,fire_ha,water_ha,fire_player_ha,water_player_ha,fire_max_ha,water_max_ha,minutes";
    const rows = ar.hours.map(([x, ...v]) => [new Date(x * 1000).toISOString(), ...v.map((n) => Math.round(n * 1000) / 1000)].join(","));
    const a = Object.assign(document.createElement("a"), {
      href: URL.createObjectURL(new Blob([[head, ...rows].join("\n") + "\n"], { type: "text/csv" })),
      download: `elements-flaechen-${region || "region"}.csv`,
    });
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  });

  rerender = render;
  $("filters").addEventListener("change", load);
  await load();
  setMode("hexes");
}

applyStatic();
langSwitch();
applyLabels();
loadCity(() => {
  // City known, and maybe the region's language instead of German.
  setFormats();
  applyStatic();
  langSwitch();
  applyLabels();
  rerender();
  drawFeedback();
});
// Legal pages keep an explicitly chosen language.
if (langChosen) for (const a of document.querySelectorAll<HTMLAnchorElement>('a[href$=".html"]')) a.href = `${a.getAttribute("href")}?lang=${lang}`;

main().catch((e) => {
  $("notice").textContent = t("dash.error");
  $("notice").hidden = false;
  console.error(e);
});
