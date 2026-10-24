// Redaktion, stage 2 (server: editor_places.rs): zones and hotspot sites on a
// map. Zones are circles (focus places in gold); hotspot sites are markers on
// junctions. Click the map for a new place, a marker to edit; markers drag.
import { ask, clean, COLL_NAME, errorOf, fmtDate, fmtTime, fromLocal, LANGS, langName, mayLeave, toast, toLocal, watchForm } from "./editor-ui";
import { t } from "./editor-i18n";
import * as maplibregl from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { basemapStyle } from "./basemap";
import { listRegions } from "./region";

maplibregl.setWorkerUrl(workerUrl);

type Zone = { id: string; name: string; lat: number; lon: number; radius_m: number | null; focus: boolean; by: string | null };
type Site = { id: string; name: string; lat: number; lon: number; node: number; by: string | null; food: string[] | null };
type Find = {
  id: string;
  collection: string;
  name: string;
  lat: number;
  lon: number;
  hidden: boolean;
  own: boolean;
  by: string | null;
  found: number;
  /** `{lang: text}`: a letter shown when found. */
  text: Record<string, string> | null;
  /** Plays only while this campaign runs; "": always. */
  campaign: string;
};
type Plan = { id: string; name: string; starts: number; ends: number; by: string | null; lat: number; lon: number };
type Places = {
  plans: Plan[];
  zones: Zone[];
  sites: Site[];
  finds: Find[];
  campaigns: string[];
  collections: string[];
  food_checked: boolean;
  auto: { lat: number; lon: number; radius_m: number };
  bounds: [number, number, number, number];
  limits: { radius: [number, number]; radius_default: number; zones: number; sites: number; food_m: number };
};
type Kind = "zone" | "site" | "find";
type Edit = {
  kind: Kind;
  id?: string;
  name: string;
  lat: number;
  lon: number;
  radius_m: number | null;
  focus: boolean;
  collection: string;
  hidden: boolean;
  text: Record<string, string>;
  campaign: string;
};

const FOCUS = "#e8a400";
const ZONE = "#5c7cfa";
const SITE = "#e8590c";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);

type Api = (path: string, init?: RequestInit) => Promise<Response>;

let api: Api;
let region = "";
/** The city's language: first row of a letter's texts. */
let primary = "de";
let data: Places | null = null;
let map: maplibregl.Map | null = null;
let mapRegion = "";
let markers: maplibregl.Marker[] = [];
let editing: Edit | null = null;
let pick: maplibregl.Popup | null = null;

/** A header row (`buttons`: an empty last column for them). */
function head(tbl: HTMLTableElement, cols: string[], buttons = false) {
  const tr = tbl.createTHead().insertRow();
  for (const c of buttons ? [...cols, ""] : cols) tr.append(Object.assign(document.createElement("th"), { textContent: c }));
}

/** A circle of `r` metres as a polygon ring. */
function circle(lon: number, lat: number, r: number): [number, number][] {
  const out: [number, number][] = [];
  const dLat = r / 111_320;
  const dLon = r / (111_320 * Math.cos((lat * Math.PI) / 180));
  for (let i = 0; i <= 64; i++) {
    const a = (i / 64) * 2 * Math.PI;
    out.push([lon + dLon * Math.cos(a), lat + dLat * Math.sin(a)]);
  }
  return out;
}

/** The map after its tab was hidden: MapLibre needs the new size, and a map
 * built while hidden (no size) still has to frame the region. */
let framed = false;
export function resizePlacesMap() {
  if (!map) return;
  map.resize();
  if (!framed && data && map.getContainer().clientWidth > 0) {
    const [w, s, e, n] = data.bounds;
    map.fitBounds([w, s, e, n], { padding: 10, duration: 0 });
    framed = true;
  }
}

export function initPlaces(apiFn: Api) {
  api = apiFn;
  addEventListener("keydown", (e) => {
    if (e.key === "Escape" && pick?.isOpen()) pick.remove();
  });
  const ff = $<HTMLSelectElement>("finds-filter");
  ff.replaceChildren(new Option(t("pl.colls.all"), ""), ...Object.entries(COLL_NAME).map(([k, v]) => new Option(v, k)));
  ff.onchange = finds;
  $("plan-add").onclick = () => void addPlan();
  watchForm("place-form");
  // New places at the map's centre (then drag the marker).
  const centre = () => (map ? map.getCenter() : null);
  $("new-zone").onclick = () => {
    const c = centre();
    if (c) openZone(null, c);
  };
  $("new-site").onclick = () => {
    const c = centre();
    if (c) openSite(null, c);
  };
  $("new-find").onclick = () => {
    const c = centre();
    if (!c) return;
    input("show-finds").checked = true;
    input("show-finds").dispatchEvent(new Event("change"));
    openFind(null, c);
  };
  input("finds-search").oninput = () => finds();
  $("place-cancel").onclick = async () => {
    if (await mayLeave("place-form")) close();
  };
  $("place-save").onclick = () => void save(false);
  $("place-delete").onclick = () => void save(true);
  $("place-letter-here").onclick = () => {
    const f = editing?.kind === "find" && editing.id ? data?.finds.find((x) => x.id === editing!.id) : undefined;
    if (f) openLetter(f);
  };
  input("place-name").oninput = draw;
  input("place-radius").oninput = draw;
  input("place-focus").onchange = draw;
  input("show-finds").onchange = draw;
}

export async function loadPlaces(slug: string, lang = "de") {
  region = slug;
  primary = lang;
  close();
  const res = await api(`/api/ops/places?region=${encodeURIComponent(region)}`).catch(() => null);
  if (!res?.ok) {
    $("places-card").hidden = true;
    return;
  }
  data = (await res.json()) as Places;
  $("places-card").hidden = false;
  await ensureMap();
  draw();
  lists();
}

async function ensureMap() {
  if (!data) return;
  if (map && mapRegion === region) return;
  map?.remove();
  mapRegion = region;
  const [w, s, e, n] = data.bounds;
  map = new maplibregl.Map({
    container: "places-map",
    style: await basemapStyle("grayscale", false, region),
    bounds: [w, s, e, n],
    fitBoundsOptions: { padding: 10 },
  });
  framed = map.getContainer().clientWidth > 0;
  await new Promise<void>((r) => map!.on("load", () => r()));
  const empty = { type: "FeatureCollection" as const, features: [] };
  // The region's boundary (as the app knows it), under the places.
  map.addSource("outline", { type: "geojson", data: empty });
  map.addLayer({ id: "outline", type: "line", source: "outline", paint: { "line-color": "#495057", "line-width": 2, "line-dasharray": [3, 2] } });
  const shown = region;
  void listRegions()
    .then((list) => {
      const r = list.find((x) => x.slug === shown);
      if (r?.outline && map && mapRegion === shown) (map.getSource("outline") as maplibregl.GeoJSONSource).setData({ type: "Feature", properties: {}, geometry: r.outline });
    })
    .catch(() => {});
  map.addSource("zones", { type: "geojson", data: empty });
  map.addLayer({
    id: "zones-fill",
    type: "fill",
    source: "zones",
    paint: { "fill-color": ["get", "color"], "fill-opacity": ["case", ["get", "auto"], 0.04, 0.15] },
  });
  map.addLayer({
    id: "zones-line",
    type: "line",
    source: "zones",
    paint: { "line-color": ["get", "color"], "line-width": ["case", ["get", "sel"], 3, 1.5], "line-dasharray": ["case", ["get", "auto"], ["literal", [2, 2]], ["literal", [1, 0]]] },
  });
  map.addSource("labels", { type: "geojson", data: empty });
  map.addLayer({
    id: "labels",
    type: "symbol",
    source: "labels",
    layout: { "text-field": ["get", "name"], "text-size": 12, "text-font": ["Noto Sans Medium"], "text-offset": [0, 1.2] },
    paint: { "text-color": "#212529", "text-halo-color": "#fff", "text-halo-width": 1.5 },
  });
  map.on("click", (ev) => {
    // A click on a marker is handled by the marker.
    if ((ev.originalEvent.target as HTMLElement).closest(".place-marker")) return;
    // With the choice open, a click elsewhere just closes it.
    if (pick?.isOpen()) return void pick.remove();
    offerNew(ev.lngLat);
  });
}

/** Map state from `data` and the open form. */
function draw() {
  if (!map || !data) return;
  const d = data;
  const form = editing ? formValues() : null;
  const zones = d.zones.map((z) => (form?.kind === "zone" && form.id === z.id ? { ...z, ...form, radius_m: form.radius_m } : z));
  if (form?.kind === "zone" && !form.id) zones.push({ id: "", by: null, ...form });
  const sites = d.sites.map((s) => (form?.kind === "site" && form.id === s.id ? { ...s, name: form.name, lat: form.lat, lon: form.lon } : s));
  if (form?.kind === "site" && !form.id) sites.push({ id: "", node: 0, by: null, food: null, name: form.name, lat: form.lat, lon: form.lon });
  const poly = (lon: number, lat: number, r: number, props: Record<string, unknown>) => ({
    type: "Feature" as const,
    properties: props,
    geometry: { type: "Polygon" as const, coordinates: [circle(lon, lat, r)] },
  });
  const feats = zones.map((z) =>
    poly(z.lon, z.lat, z.radius_m ?? d.limits.radius_default, {
      color: z.focus ? FOCUS : ZONE,
      auto: false,
      sel: !!form && form.kind === "zone" && (form.id ?? "") === z.id,
    }),
  );
  // Without the city's own sites the game picks some in this area.
  if (!sites.length) feats.push(poly(d.auto.lon, d.auto.lat, d.auto.radius_m, { color: SITE, auto: true, sel: false }));
  (map.getSource("zones") as maplibregl.GeoJSONSource).setData({ type: "FeatureCollection", features: feats });
  (map.getSource("labels") as maplibregl.GeoJSONSource).setData({
    type: "FeatureCollection",
    features: zones.map((z) => ({ type: "Feature", properties: { name: z.name }, geometry: { type: "Point", coordinates: [z.lon, z.lat] } })),
  });
  for (const m of markers) m.remove();
  markers = [];
  const add = (kind: Kind, id: string, lon: number, lat: number, title: string, warn: boolean) => {
    const el = document.createElement("div");
    el.className = `place-marker ${kind}${warn ? " warn" : ""}`;
    el.title = title;
    const m = new maplibregl.Marker({ element: el, draggable: true }).setLngLat([lon, lat]).addTo(map!);
    const openIt = () => {
      if (kind === "zone") openZone(d.zones.find((z) => z.id === id) ?? null);
      else if (kind === "site") openSite(d.sites.find((s) => s.id === id) ?? null);
      else openFind(d.finds.find((f) => f.id === id) ?? null);
    };
    el.addEventListener("click", (ev) => {
      ev.stopPropagation();
      openIt();
    });
    m.on("dragend", () => {
      const p = m.getLngLat();
      if (!editing || editing.kind !== kind || (editing.id ?? "") !== id) openIt();
      setPos(p.lat, p.lng);
    });
    markers.push(m);
  };
  for (const z of zones) add("zone", z.id, z.lon, z.lat, z.name, false);
  for (const s of sites) add("site", s.id, s.lon, s.lat, s.name, (s.food?.length ?? 0) > 0);
  // Finds only when shown (Toronto has hundreds) or the one being edited.
  const finds = d.finds.map((f) => (form?.kind === "find" && form.id === f.id ? { ...f, ...form } : f));
  if (form?.kind === "find" && !form.id) finds.push({ id: "", own: true, by: null, found: 0, ...form });
  for (const f of finds) {
    const open = form?.kind === "find" && (form.id ?? "") === f.id;
    if (!open && !input("show-finds").checked) continue;
    add("find", f.id, f.lon, f.lat, `${COLL_NAME[f.collection] ?? f.collection}: ${f.name}`, false);
    const el = markers[markers.length - 1].getElement();
    el.dataset.c = f.collection;
    if (f.hidden) el.classList.add("hidden-find");
  }
}

function offerNew(at: maplibregl.LngLat) {
  pick?.remove();
  const box = document.createElement("div");
  box.className = "place-pick";
  const b = (label: string, fn: () => void) => {
    const x = document.createElement("button");
    x.type = "button";
    x.textContent = label;
    x.onclick = () => {
      pick?.remove();
      fn();
    };
    box.append(x);
  };
  b(t("pl.here.zone"), () => openZone(null, at));
  b(t("pl.here.site"), () => openSite(null, at));
  b(t("pl.here.find"), () => openFind(null, at));
  b(t("ui.cancel"), () => {});
  box.lastElementChild!.classList.add("quiet");
  pick = new maplibregl.Popup({ closeButton: true, closeOnClick: false }).setLngLat(at).setDOMContent(box).addTo(map!);
}

const blank = { radius_m: null, focus: false, collection: "", hidden: false, text: {}, campaign: "" };
/** The form holds a new letter laid at an existing find (openLetter). */
let asLetter = false;

function openZone(z: Zone | null, at?: maplibregl.LngLat) {
  clean("place-form");
  asLetter = false;
  editing = z
    ? { ...blank, kind: "zone", id: z.id, name: z.name, lat: z.lat, lon: z.lon, radius_m: z.radius_m, focus: z.focus }
    : { ...blank, kind: "zone", name: "", lat: at!.lat, lon: at!.lng, radius_m: 150, focus: true };
  fill();
}

function openSite(s: Site | null, at?: maplibregl.LngLat) {
  clean("place-form");
  asLetter = false;
  editing = s
    ? { ...blank, kind: "site", id: s.id, name: s.name, lat: s.lat, lon: s.lon }
    : { ...blank, kind: "site", name: "", lat: at!.lat, lon: at!.lng };
  fill();
}

function openFind(f: Find | null, at?: maplibregl.LngLat) {
  clean("place-form");
  asLetter = false;
  editing = f
    ? { ...blank, kind: "find", id: f.id, name: f.name, lat: f.lat, lon: f.lon, collection: f.collection, hidden: f.hidden, text: { ...(f.text ?? {}) }, campaign: f.campaign }
    : { ...blank, kind: "find", name: "", lat: at!.lat, lon: at!.lng, collection: "fountains" };
  fill();
}

/** A letter at an existing find: a new find at the same spot, bound to the
 * newest campaign, so it is found only while that runs (like every find it
 * stays unseen until a claim nearby uncovers it); the find itself (a fountain,
 * a gate) stays as it is. */
function openLetter(f: Find) {
  clean("place-form");
  asLetter = true;
  editing = {
    ...blank,
    kind: "find",
    name: t("pl.letter.name", { name: f.name }),
    lat: f.lat,
    lon: f.lon,
    collection: f.collection,
    campaign: data?.campaigns[0] ?? "",
  };
  fill();
  $<HTMLTextAreaElement>(`place-text-${primary}`)?.focus();
}

/** A letter's texts: the city's language first, the others after it. */
function drawLetter(text: Record<string, string>) {
  const langs = [primary, ...LANGS.filter((l) => l !== primary)];
  const host = $("place-letter-langs");
  host.replaceChildren(
    ...langs.flatMap((l) => {
      const label = Object.assign(document.createElement("span"), { textContent: langName(l) + (l === primary ? " *" : "") });
      const ta = Object.assign(document.createElement("textarea"), { id: `place-text-${l}`, maxLength: 1500, rows: 4, value: text[l] ?? "" });
      ta.oninput = letterCount;
      return [label, ta];
    }),
  );
  letterCount();
}

function letterCount() {
  const n = LANGS.filter((l) => $<HTMLTextAreaElement>(`place-text-${l}`)?.value.trim()).length;
  $("place-letter-count").textContent = n ? t("mo.count", { n, m: LANGS.length }) : "";
}

function fill() {
  if (!editing || !data) return;
  const e = editing;
  const zone = e.kind === "zone";
  const find = e.kind === "find";
  const f = find ? data.finds.find((x) => x.id === e.id) : undefined;
  $("place-title").textContent = t(`pl.${e.id ? "edit" : "create"}.${e.kind}`);
  $<HTMLSelectElement>("place-collection").replaceChildren(...data.collections.map((c) => new Option(COLL_NAME[c] ?? c, c)));
  $<HTMLSelectElement>("place-collection").value = e.collection;
  input("place-hidden").checked = e.hidden;
  const camp = $<HTMLSelectElement>("place-campaign");
  camp.replaceChildren(new Option(t("pl.campaign.none"), ""), ...data.campaigns.map((c) => new Option(c, c)));
  if (e.campaign && !data.campaigns.includes(e.campaign)) camp.append(new Option(e.campaign, e.campaign));
  camp.value = e.campaign;
  drawLetter(e.text);
  ($("place-letter") as HTMLDetailsElement).open = asLetter || Object.values(e.text).some(Boolean);
  $("place-letter-here").hidden = !(find && e.id);
  $("place-letter").hidden = !find;
  $("place-find-fields").hidden = !find;
  $("place-find-note").hidden = !find;
  $("place-find-note").textContent = !f
    ? t(asLetter ? "pl.letter.new.note" : "pl.find.new.note")
    : `${t("pl.find.found", { origin: t(f.own ? "pl.find.own" : "pl.find.osm"), n: f.found })} ${f.own && !f.found ? "" : t("pl.find.hide.note")}`;
  input("place-name").value = e.name;
  input("place-radius").value = e.radius_m == null ? "" : String(e.radius_m);
  input("place-radius").placeholder = t("pl.radius.ph", { m: data.limits.radius_default });
  input("place-focus").checked = e.focus;
  $("place-zone-fields").hidden = !zone;
  $("place-site-note").hidden = e.kind !== "site";
  $("place-delete").hidden = !e.id || (find && !(f?.own && !f.found));
  $("place-error").textContent = "";
  setPos(e.lat, e.lon);
  $("place-form").hidden = false;
  input("place-name").focus();
}

function setPos(lat: number, lon: number) {
  if (!editing) return;
  editing.lat = lat;
  editing.lon = lon;
  $("place-pos").textContent = t("pl.pos", { pos: `${lat.toFixed(5)}, ${lon.toFixed(5)}` });
  draw();
}

function formValues(): Edit {
  const e = editing!;
  const r = input("place-radius").value.trim();
  return {
    ...e,
    name: input("place-name").value,
    radius_m: e.kind === "zone" && r ? Number(r) : null,
    focus: e.kind === "zone" && input("place-focus").checked,
    collection: e.kind === "find" ? $<HTMLSelectElement>("place-collection").value : "",
    hidden: e.kind === "find" && input("place-hidden").checked,
    text: e.kind === "find" ? Object.fromEntries(LANGS.map((l) => [l, $<HTMLTextAreaElement>(`place-text-${l}`)?.value.trim() ?? ""]).filter(([, v]) => v)) : {},
    campaign: e.kind === "find" ? $<HTMLSelectElement>("place-campaign").value : "",
  };
}

function close() {
  clean("place-form");
  editing = null;
  pick?.remove();
  $("place-form").hidden = true;
  draw();
}

async function save(del: boolean) {
  if (!editing) return;
  const v = formValues();
  if (del && !(await ask(t("pl.delete.ask", { name: v.name }), t("ui.delete"), true))) return;
  const body = del ? { kind: v.kind, id: v.id, delete: true } : { ...v, collection: v.kind === "find" ? v.collection : undefined };
  const res = await api(`/api/ops/places?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("place-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("place-form");
  toast(t(del ? "pl.deleted" : "pl.saved", { name: v.name }));
  await loadPlaces(region);
}

function lists() {
  if (!data) return;
  const d = data;
  // Zones.
  const zt = document.createElement("table");
  zt.className = "ed-table";
  head(zt, [t("pl.th.zone"), t("pl.th.radius"), t("pl.th.focus"), t("pl.th.last")]);
  for (const z of d.zones) {
    const tr = zt.insertRow();
    const name = tr.insertCell();
    const a = document.createElement("button");
    a.type = "button";
    a.className = "linkish";
    a.textContent = z.name;
    a.onclick = () => {
      openZone(z);
      map?.easeTo({ center: [z.lon, z.lat] });
    };
    name.append(a);
    tr.insertCell().textContent = z.radius_m == null ? t("pl.radius.default", { m: d.limits.radius_default }) : `${z.radius_m} m`;
    tr.insertCell().textContent = z.focus ? t("ui.yes") : "";
    tr.insertCell().textContent = z.by ?? t("pl.import");
  }
  $("zones-list").replaceChildren(d.zones.length ? zt : document.createTextNode(t("pl.zones.none")));
  // Hotspot sites.
  const st = document.createElement("table");
  st.className = "ed-table";
  head(st, [t("pl.th.site"), t("pl.th.food", { m: d.limits.food_m }), t("pl.th.last")]);
  for (const s of d.sites) {
    const tr = st.insertRow();
    const name = tr.insertCell();
    const a = document.createElement("button");
    a.type = "button";
    a.className = "linkish";
    a.textContent = s.name;
    a.onclick = () => {
      openSite(s);
      map?.easeTo({ center: [s.lon, s.lat] });
    };
    name.append(a);
    tr.insertCell().textContent = s.food == null ? "–" : s.food.length ? s.food.join(", ") : t("pl.food.none");
    tr.insertCell().textContent = s.by ?? "";
  }
  $("sites-list").replaceChildren(
    d.sites.length
      ? st
      : document.createTextNode(t("pl.sites.none", { m: Math.round(d.auto.radius_m) })),
  );
  // Bias: hotspots mostly at restaurants and bars draw a narrow crowd.
  const withFood = d.sites.filter((s) => (s.food?.length ?? 0) > 0).length;
  const warn = $("sites-warn");
  warn.hidden = !(d.sites.length >= 3 && withFood / d.sites.length >= 0.5);
  warn.textContent = t("pl.sites.warn", { n: withFood, m: d.sites.length });
  $("sites-food-note").hidden = d.food_checked;
  finds();
  plans();
}


/** Planned hotspots: a site and a window; the game switches them on itself. */
function plans() {
  if (!data) return;
  const d = data;
  const sel = $<HTMLSelectElement>("plan-site");
  sel.replaceChildren(...d.sites.map((s) => new Option(s.name, s.id)));
  $("plan-form").hidden = !d.sites.length;
  $("plan-none").hidden = !!d.sites.length;
  if (!input("plan-starts").value) {
    const t = Math.ceil(Date.now() / 3_600_000) * 3600 + 86_400;
    input("plan-starts").value = toLocal(t);
    input("plan-ends").value = toLocal(t + 4 * 3600);
  }
  const now = Date.now() / 1000;
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  head(tbl, [t("pl.th.place"), t("pl.th.period"), t("pl.th.status"), t("pl.th.by")], true);
  for (const p of d.plans) {
    const tr = tbl.insertRow();
    const st = now < p.starts ? "planned" : now < p.ends ? "live" : "over";
    if (st === "over") tr.className = "muted";
    tr.insertCell().textContent = p.name;
    tr.insertCell().textContent = `${fmtDate(p.starts)} – ${fmtTime(p.ends)}`;
    tr.insertCell().textContent = t(`st.${st}`);
    tr.insertCell().textContent = p.by ?? "";
    const td = tr.insertCell();
    if (st !== "over") {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = t("ui.delete");
      b.onclick = async () => {
        if (!(await ask(st === "live" ? t("pl.plan.delete.live") : t("pl.plan.delete.ask", { name: p.name }), t("ui.delete"), true))) return;
        const res = await api(`/api/ops/places?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify({ kind: "plan", id: p.id, delete: true }) }).catch(() => null);
        if (!res?.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
        else toast(t("pl.plan.deleted"));
        await loadPlaces(region);
      };
      td.append(b);
    }
  }
  $("plans-list").replaceChildren(d.plans.length ? tbl : document.createTextNode(t("pl.plans.none")));
}

async function addPlan() {
  const body = {
    kind: "plan",
    site: $<HTMLSelectElement>("plan-site").value,
    starts: fromLocal(input("plan-starts").value),
    ends: fromLocal(input("plan-ends").value),
  };
  const res = await api(`/api/ops/places?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("plan-error").textContent = await errorOf(res, t("pl.plan.fail"));
    return;
  }
  $("plan-error").textContent = "";
  toast(t("pl.planned"));
  await loadPlaces(region);
}

/** Finds by collection; a click opens one, the button hides or shows it. */
function finds() {
  if (!data) return;
  const d = data;
  const want = $<HTMLSelectElement>("finds-filter").value;
  const words = input("finds-search").value.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const list = d.finds.filter((f) => (!want || f.collection === want) && words.every((w) => f.name.toLowerCase().includes(w)));
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  head(tbl, [t("pl.th.find"), t("pl.th.coll"), t("pl.th.origin"), t("pl.th.found")], true);
  for (const f of list) {
    const tr = tbl.insertRow();
    if (f.hidden) tr.className = "muted";
    const name = tr.insertCell();
    const a = document.createElement("button");
    a.type = "button";
    a.className = "linkish";
    a.textContent = f.name;
    a.onclick = () => {
      openFind(f);
      map?.easeTo({ center: [f.lon, f.lat], zoom: Math.max(map.getZoom(), 15) });
    };
    name.append(a);
    // A letter, a campaign: small marks.
    const marks = [f.text && Object.keys(f.text).length ? "📖" : "", f.campaign].filter(Boolean).join(" ");
    if (marks) name.append(" ", Object.assign(document.createElement("small"), { textContent: marks }));
    tr.insertCell().textContent = COLL_NAME[f.collection] ?? f.collection;
    tr.insertCell().textContent = f.own ? t("pl.origin.own", { by: f.by ?? "" }) : f.by ? t("pl.origin.osmed", { by: f.by }) : "OSM";
    tr.insertCell().textContent = String(f.found);
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = t(f.hidden ? "pl.show" : "pl.hide");
    // Acts at once, with undo in the notice.
    const setHidden = async (hidden: boolean) => {
      const res = await api(`/api/ops/places?region=${encodeURIComponent(region)}`, {
        method: "POST",
        body: JSON.stringify({ kind: "find", id: f.id, name: f.name, lat: f.lat, lon: f.lon, collection: f.collection, hidden, text: f.text ?? {}, campaign: f.campaign }),
      }).catch(() => null);
      await loadPlaces(region);
      return res?.ok ? null : await errorOf(res, t("ui.failed"));
    };
    b.onclick = async () => {
      const err = await setHidden(!f.hidden);
      if (err) toast(err, "error");
      else toast(t(f.hidden ? "pl.shown.again" : "pl.hidden.toast", { name: f.name }), "ok", () => void setHidden(f.hidden));
    };
    tr.insertCell().append(b);
  }
  const shown = d.finds.filter((f) => !f.hidden).length;
  $("finds-count").textContent = t("pl.count", { shown, hidden: d.finds.length - shown });
  $("finds-list").replaceChildren(list.length ? tbl : document.createTextNode(t("pl.finds.none")));
}
