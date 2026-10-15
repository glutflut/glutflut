// Time-lapse of the last 72 hours: the globe files (GFL1, docs/data-files.md)
// as hexagons, bin by bin (15 minutes). Same data the globe shows; quiet
// regions only change once a day there, so no lone player's walk shows.
import * as maplibregl from "maplibre-gl";
import { cellToBoundary } from "h3-js";
import { lang, t } from "./i18n";
import { COLORS } from "./geo";
import { $ } from "./page";

interface Cell {
  /** Run starts (bin) and the shares (0..255) from there on. */
  start: Uint16Array;
  fire: Uint8Array;
  water: Uint8Array;
  front: Uint8Array;
}

interface Lapse {
  startMin: number;
  binMin: number;
  bins: number;
  cells: Cell[];
  geo: GeoJSON.FeatureCollection;
}

/** Game layers hidden while the time-lapse plays (the hexagons replace them). */
const HIDE = ["edges", "edges-glow", "edges-core", "nodes", "flow", "blocks", "blocks-players", "overview", "natural", "mottos"];

function parse(buf: ArrayBuffer): Lapse {
  const v = new DataView(buf);
  if (String.fromCharCode(v.getUint8(0), v.getUint8(1), v.getUint8(2), v.getUint8(3)) !== "GFL1") throw new Error("not GFL1");
  const flags = v.getUint8(5);
  const binMin = v.getUint16(6, true);
  const bins = v.getUint16(8, true);
  const startMin = v.getUint32(12, true);
  const count = v.getUint32(16, true);
  const step = flags & 1 ? 8 : 6;
  const cells: Cell[] = [];
  const features: GeoJSON.Feature[] = [];
  let o = 24;
  for (let i = 0; i < count; i++) {
    const h3 = v.getBigUint64(o, true).toString(16);
    const runs = v.getUint16(o + 10, true);
    o += 12;
    const c: Cell = { start: new Uint16Array(runs), fire: new Uint8Array(runs), water: new Uint8Array(runs), front: new Uint8Array(runs) };
    for (let r = 0; r < runs; r++, o += step) {
      c.start[r] = v.getUint16(o, true);
      c.fire[r] = v.getUint8(o + 2);
      c.water[r] = v.getUint8(o + 3);
      c.front[r] = v.getUint8(o + 4);
    }
    const ring = cellToBoundary(h3, true);
    features.push({ type: "Feature", id: cells.length, properties: {}, geometry: { type: "Polygon", coordinates: [ring] } });
    cells.push(c);
  }
  return { startMin, binMin, bins, cells, geo: { type: "FeatureCollection", features } };
}

/** The run of `c` in force at `bin` (-1: no data yet). */
function runAt(c: Cell, bin: number): number {
  let r = -1;
  for (let i = 0; i < c.start.length && c.start[i] <= bin; i++) r = i;
  return r;
}

export function setupTimelapse(map: maplibregl.Map, slug: () => string) {
  let lapse: Lapse | null = null;
  let bin = 0;
  let last = 0;
  let timer = 0;
  const hidden: string[] = [];
  /** The view before the time-lapse zoomed out, restored on close. */
  let before: maplibregl.CameraOptions | null = null;
  const panel = $("lapse");
  const slider = $("lapse-range") as HTMLInputElement;
  const play = $("lapse-play");
  const when = $("lapse-when");

  async function load(): Promise<Lapse> {
    const m = await fetch("/api/marble/manifest.json").then((r) => (r.ok ? r.json() : Promise.reject(r.status)));
    const reg = (m.regions as { slug: string; files: Record<string, string> }[]).find((r) => r.slug === slug());
    const file = reg?.files["9"] ?? reg?.files["8"];
    if (!file) throw new Error("no time-lapse here");
    return parse(await fetch(`/api/marble/${file}`).then((r) => r.arrayBuffer()));
  }

  function draw() {
    if (!lapse) return;
    for (let i = 0; i < lapse.cells.length; i++) {
      const c = lapse.cells[i];
      const r = runAt(c, bin);
      const [f, w, fr] = r < 0 ? [0, 0, 0] : [c.fire[r], c.water[r], c.front[r]];
      map.setFeatureState({ source: "lapse", id: i }, { f, w, fr });
    }
    const at = new Date((lapse.startMin + bin * lapse.binMin) * 60_000);
    when.textContent = at.toLocaleString(lang, { weekday: "short", hour: "2-digit", minute: "2-digit" });
    slider.value = String(bin);
  }

  function stop() {
    clearInterval(timer);
    timer = 0;
    play.textContent = "▶";
    play.setAttribute("aria-label", t("lapse.play"));
  }

  function start() {
    if (!lapse) return;
    if (bin >= last) bin = Math.max(0, last - 96); // from 24 h ago
    play.textContent = "❚❚";
    play.setAttribute("aria-label", t("lapse.pause"));
    timer = window.setInterval(() => {
      if (bin >= last) return stop();
      bin++;
      draw();
    }, 120);
  }

  async function open() {
    panel.hidden = false;
    when.textContent = t("lapse.loading");
    try {
      lapse ??= await load();
    } catch {
      when.textContent = t("lapse.none");
      return;
    }
    // The last bin that has started (the window runs ahead to its end).
    last = Math.min(lapse.bins - 1, Math.floor((Date.now() / 60_000 - lapse.startMin) / lapse.binMin));
    slider.max = String(last);
    if (!map.getSource("lapse")) {
      map.addSource("lapse", { type: "geojson", data: lapse.geo });
      const st = (k: string): maplibregl.ExpressionSpecification => ["coalesce", ["feature-state", k], 0];
      const share: maplibregl.ExpressionSpecification = ["/", ["max", st("f"), st("w"), st("fr")], 255];
      map.addLayer({
        id: "lapse",
        type: "fill",
        source: "lapse",
        paint: {
          "fill-color": [
            "case",
            ["all", [">=", st("f"), st("w")], [">=", st("f"), st("fr")]],
            COLORS.fire,
            [">=", st("w"), st("fr")],
            COLORS.water,
            COLORS.front,
          ],
          "fill-opacity": ["*", 0.65, ["min", 1, ["*", 1.6, share]]],
          "fill-outline-color": "rgba(22,24,29,0.5)",
        },
      });
    }
    for (const id of HIDE)
      if (map.getLayer(id) && map.getLayoutProperty(id, "visibility") !== "none") {
        map.setLayoutProperty(id, "visibility", "none");
        hidden.push(id);
      }
    map.setLayoutProperty("lapse", "visibility", "visible");
    // A hexagon is ~350 m across: show the town, not one street.
    before ??= { center: map.getCenter(), zoom: map.getZoom(), pitch: map.getPitch(), bearing: map.getBearing() };
    if (map.getZoom() > 13.5) map.easeTo({ zoom: 13, pitch: 0, duration: 800 });
    document.body.classList.add("lapse");
    bin = last;
    draw();
    start();
  }

  function close() {
    stop();
    panel.hidden = true;
    document.body.classList.remove("lapse");
    if (map.getLayer("lapse")) map.setLayoutProperty("lapse", "visibility", "none");
    for (const id of hidden.splice(0)) map.setLayoutProperty(id, "visibility", "visible");
    if (before) map.easeTo({ ...before, duration: 800 });
    before = null;
  }

  play.onclick = () => (timer ? stop() : start());
  slider.oninput = () => {
    stop();
    bin = Number(slider.value);
    draw();
  };
  $("lapse-close").onclick = close;
  $("nav-lapse").onclick = () => (panel.hidden ? void open() : close());
  return { open, close, on: () => !panel.hidden };
}
