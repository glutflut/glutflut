// Weather on the map from published weather files
// (docs/data-files.md "Weather"): XYZ tiles per
// source, 8-bit greyscale PNGs of byte codes (255 no data) that a protocol per
// product colours here; one raster layer per source under the game layers, the
// tile address swapped for each new 15-minute frame. Rain (radar, OPERA/MRMS)
// and cloud cover (satellite) each have a button; both off by default, the
// buttons remember it. Clouds lie under the rain. Only where the server names
// a weather host (`weather` in /api/version, `ELEMENTS_WEATHER_URL`); without
// one the buttons stay hidden.
import * as maplibregl from "maplibre-gl";
import { t, type Key } from "./i18n";
import { $ } from "./page";
import { KEYS, load, save, type StoreKey } from "./store";

const REFRESH_MS = 5 * 60_000;

type Bounds = [number, number, number, number];
type Manifest = {
  format: string;
  source: string;
  min_zoom: number;
  max_zoom: number;
  license?: string;
  attribution?: string;
  bounds?: Bounds;
  frames: { time: string; kind: string; dir: string }[];
};
// `{format, sources: [{source, bounds, max_zoom, manifest, priority?, kind?, masked_by?}]}`;
// `kind` "radar" (default) or "satellite"; a higher `priority` is the better
// source; `masked_by`: better sources whose data this one's tiles leave out.
type Listed = {
  source: string;
  bounds?: Bounds;
  manifest?: string;
  priority?: number;
  kind?: string;
  masked_by?: string[];
};

/** Byte code -> RGBA, 256 entries. */
function ramp(f: (v: number) => [number, number, number, number] | null): Uint8ClampedArray {
  const out = new Uint8ClampedArray(256 * 4);
  for (let v = 0; v < 256; v++) out.set(f(v) ?? [0, 0, 0, 0], v * 4);
  return out;
}

/** Rain: 0 dry, 1..253 rain (0.1 .. ~320 mm/h, 72 steps a decade); blue
 *  getting deeper and more opaque with the rate. */
const RAIN = ramp((v) => {
  if (v === 0 || v > 253) return null;
  const mm = 0.1 * 10 ** ((v - 1) / 72);
  // 0.1 -> 0, 1 -> .33, 10 -> .67, 100 -> 1
  const k = Math.min(1, Math.max(0, (Math.log10(mm) + 1) / 3));
  const light = [116, 192, 252], deep = [24, 100, 171], violet = [132, 94, 247];
  const [a, b, f] = k < 0.67 ? [light, deep, k / 0.67] : [deep, violet, (k - 0.67) / 0.33];
  const c = a.map((x, i) => Math.round(x + (b[i] - x) * f));
  return [c[0], c[1], c[2], Math.round(255 * (0.35 + 0.5 * k))];
});

/** Clouds: 0..100 % cover; a white veil, thicker with more cloud. */
const CLOUDS = ramp((v) => (v === 0 || v > 100 ? null : [226, 232, 240, Math.round(255 * 0.4 * (v / 100))]));

type Product = {
  /** Protocol name, layer id prefix. */
  name: string;
  path: string;
  ramp: Uint8ClampedArray;
  fallback: string[];
  key: StoreKey;
  button: string;
  credit: string;
  text: { at: Key; satellite: Key; none: Key; failed: Key };
  /** Layers to insert before (the first that exists). */
  before: string[];
};

const PRODUCTS: Product[] = [
  {
    name: "clouds",
    path: "clouds/v1/tiles",
    ramp: CLOUDS,
    fallback: [],
    key: KEYS.clouds,
    button: "nav-clouds",
    credit: "clouds-credit",
    text: { at: "clouds.at", satellite: "clouds.at", none: "clouds.none", failed: "clouds.failed" },
    before: ["radar-opera", "radar-mrms", "overview"],
  },
  {
    name: "radar",
    path: "radar/v2/tiles",
    ramp: RAIN,
    fallback: ["opera", "mrms"],
    key: KEYS.radar,
    button: "nav-radar",
    credit: "radar-credit",
    text: { at: "radar.at", satellite: "radar.at.satellite", none: "radar.none", failed: "radar.failed" },
    before: ["overview"],
  },
];

let empty: ArrayBuffer | null = null;
async function emptyTile(): Promise<ArrayBuffer> {
  if (!empty) empty = await (await new OffscreenCanvas(1, 1).convertToBlob({ type: "image/png" })).arrayBuffer();
  return empty;
}

const registered = new Set<string>();
/** `<name>://<host>/<path>`: fetch the greyscale tile (over `scheme`), colour it; missing = empty. */
function register(p: Product, scheme: string) {
  if (registered.has(p.name)) return;
  registered.add(p.name);
  maplibregl.addProtocol(p.name, async (params, abort) => {
    const res = await fetch(params.url.replace(`${p.name}://`, `${scheme}//`), { signal: abort.signal });
    if (!res.ok) return { data: await emptyTile() };
    const img = await createImageBitmap(await res.blob());
    const c = new OffscreenCanvas(img.width, img.height);
    const g = c.getContext("2d", { willReadFrequently: true })!;
    g.drawImage(img, 0, 0);
    const d = g.getImageData(0, 0, img.width, img.height);
    const px = d.data;
    for (let i = 0; i < px.length; i += 4) {
      const v = px[i] * 4;
      px[i] = p.ramp[v];
      px[i + 1] = p.ramp[v + 1];
      px[i + 2] = p.ramp[v + 2];
      px[i + 3] = p.ramp[v + 3];
    }
    g.putImageData(d, 0, 0);
    return { data: await (await c.convertToBlob({ type: "image/png" })).arrayBuffer() };
  });
}

/** Source lists and manifests, kept a minute (the map re-checks after every move). */
const recent = new Map<string, { at: number; value: Promise<unknown> }>();
const json = <T,>(url: string): Promise<T | null> => {
  const hit = recent.get(url);
  if (hit && Date.now() - hit.at < 60_000) return hit.value as Promise<T | null>;
  const value = fetch(url, { cache: "no-cache" })
    .then((res) => (res.ok ? (res.json() as Promise<T>) : null))
    .catch(() => null);
  recent.set(url, { at: Date.now(), value });
  return value;
};

const touches = (b: Bounds | undefined, v: maplibregl.LngLatBounds) =>
  !b || !(b[2] < v.getWest() || b[0] > v.getEast() || b[3] < v.getSouth() || b[1] > v.getNorth());

function setupProduct(map: maplibregl.Map, toast: (text: string, ms?: number) => void, p: Product, base: URL) {
  const btn = $(p.button);
  const credit = $(p.credit);
  btn.hidden = false;
  const root = `${base.host}${base.pathname.replace(/\/$/, "")}/${p.path}`;
  const host = `${base.protocol}//${root}`;
  let on = load(p.key) === "1";
  let timer: number | undefined;
  /** Source -> frame directory on the map. */
  const shown = new Map<string, string>();

  function drop(s: string) {
    const id = `${p.name}-${s}`;
    if (map.getLayer(id)) map.removeLayer(id);
    if (map.getSource(id)) map.removeSource(id);
    shown.delete(s);
  }

  function clear() {
    [...shown.keys()].forEach(drop);
    credit.hidden = true;
  }

  async function refresh(tell = false) {
    if (!on) return;
    register(p, base.protocol);
    const listed = await json<{ sources?: Listed[] }>(`${host}/sources.json`);
    const list: Listed[] = listed?.sources ?? p.fallback.map((source) => ({ source }));
    const view = map.getBounds();
    // Overlapping sources would double the colour: a source a better one
    // covers entirely is left out, unless its tiles already leave that one's
    // data out (`masked_by`, then it fills the better one's gaps); the rest go
    // better-on-top.
    const covers = (b: Bounds | undefined) =>
      !!b && b[0] <= view.getWest() && b[1] <= view.getSouth() && b[2] >= view.getEast() && b[3] >= view.getNorth();
    const prio = (x: Listed) => x.priority ?? 0;
    const use = list
      .filter((x) => touches(x.bounds, view))
      .filter(
        (x, _, all) => !all.some((y) => prio(y) > prio(x) && covers(y.bounds) && !x.masked_by?.includes(y.source)),
      )
      .sort((a, b) => prio(a) - prio(b));
    [...shown.keys()].filter((s) => !use.some((x) => x.source === s)).forEach(drop);
    const latest: { time: string; credit: string; satellite: boolean }[] = [];
    for (const { source, bounds, manifest, kind } of use) {
      const m = await json<Manifest>(`${host}/${manifest ?? `${source}/manifest.json`}`);
      const f = m?.frames.filter((x) => x.kind === "obs").at(-1);
      if (!m || !f || !touches(m.bounds ?? bounds, view) || !on) continue;
      const id = `${p.name}-${source}`;
      const tiles = [`${p.name}://${root}/${source}/${f.dir}/{z}/{x}/{y}.png`];
      const src = map.getSource(id) as maplibregl.RasterTileSource | undefined;
      if (!src) {
        map.addSource(id, { type: "raster", tiles, tileSize: 256, minzoom: m.min_zoom, maxzoom: m.max_zoom });
        map.addLayer(
          { id, type: "raster", source: id, paint: { "raster-opacity": 1, "raster-fade-duration": 0 } },
          p.before.find((l) => map.getLayer(l)),
        );
      } else if (shown.get(source) !== f.dir) src.setTiles(tiles);
      shown.set(source, f.dir);
      latest.push({
        time: f.time,
        credit: [m.attribution, m.license].filter(Boolean).join(", "),
        satellite: kind === "satellite",
      });
    }
    if (!latest.length) {
      clear();
      if (tell) toast(t(p.text.none), 5000);
      return;
    }
    const newest = latest.map((x) => x.time).sort().at(-1)!;
    const time = new Date(newest).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    // Time in the bottom line, so the buttons don't move; the sources
    // open on a tap of ⓘ.
    const at = t(latest.every((x) => x.satellite) ? p.text.satellite : p.text.at, { time });
    const full = document.createElement("span");
    full.textContent = `: ${latest.map((x) => x.credit).join("; ")}`;
    full.hidden = !credit.classList.contains("open");
    const more = document.createElement("a");
    more.href = "#";
    more.textContent = "ⓘ";
    more.title = t("wx.sources");
    more.onclick = (e) => {
      e.preventDefault();
      full.hidden = !full.hidden;
      credit.classList.toggle("open", !full.hidden);
    };
    credit.replaceChildren(` · ${at} `, more, full);
    credit.hidden = false;
  }

  function set(v: boolean) {
    on = v;
    save(p.key, v ? "1" : "0");
    btn.classList.toggle("on", v);
    clearInterval(timer);
    if (v) {
      void refresh(true).catch(() => toast(t(p.text.failed), 5000));
      timer = window.setInterval(() => void refresh().catch(() => {}), REFRESH_MS);
    } else clear();
  }

  btn.onclick = () => set(!on);
  // Another place (a jump to another city, panning out to sea): the sources
  // for the new view, a moment after the map comes to rest.
  let settle: number | undefined;
  map.on("moveend", () => {
    if (!on) return;
    clearTimeout(settle);
    settle = window.setTimeout(() => void refresh().catch(() => {}), 800);
  });
  // The map is ready when this is called; a remembered choice applies at once.
  set(on);
}

/** Rain radar and cloud cover layers with their buttons, if the server names a weather host. */
export async function setupRadar(map: maplibregl.Map, toast: (text: string, ms?: number) => void) {
  const v = (await fetch("/api/version")
    .then((r) => r.json())
    .catch(() => ({}))) as { weather?: string };
  let base: URL;
  try {
    base = new URL(v.weather ?? "");
  } catch {
    return;
  }
  for (const p of PRODUCTS) setupProduct(map, toast, p, base);
}
