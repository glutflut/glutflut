// A claim through a link (server `beam`, to everyone here): the permanent
// source it came from glows, a spark runs along a line to the claimed
// junction, the line fades. The own source A only glows on the player's
// device (`glow`): nobody else learns which source a link starts from.
import * as maplibregl from "maplibre-gl";
import { COLORS, GLOW } from "./geo";
import { params } from "./page";
import type { Faction } from "./protocol";

/** How long the spark runs and the line stays (ms). */
const RUN_MS = 1100;
const FADE_MS = 2000;
/** Beams at once (many players linking at the same time). */
const MAX = 6;

type Beam = { from: [number, number]; to: [number, number]; side: Faction; t0: number };

export function setupBeam(map: maplibregl.Map, gameView: boolean) {
  const c = gameView ? GLOW : COLORS;
  const empty = (): GeoJSON.FeatureCollection => ({ type: "FeatureCollection", features: [] });
  map.addSource("beam", { type: "geojson", data: empty() });
  map.addLayer({
    id: "beam-glow",
    type: "line",
    source: "beam",
    filter: ["==", ["geometry-type"], "LineString"],
    layout: { "line-cap": "round" },
    paint: { "line-color": ["match", ["get", "side"], "fire", c.fire, c.water], "line-width": 18, "line-blur": 10, "line-opacity": ["*", 0.9, ["get", "a"]] },
  });
  map.addLayer({
    id: "beam",
    type: "line",
    source: "beam",
    filter: ["==", ["geometry-type"], "LineString"],
    layout: { "line-cap": "round" },
    paint: { "line-color": "#fff", "line-width": 3.5, "line-opacity": ["get", "a"] },
  });
  map.addLayer({
    id: "beam-spark",
    type: "circle",
    source: "beam",
    filter: ["==", ["geometry-type"], "Point"],
    paint: {
      "circle-color": "#fff",
      "circle-radius": 6,
      "circle-blur": 0.3,
      "circle-stroke-width": 6,
      "circle-stroke-color": ["match", ["get", "side"], "fire", c.fire, c.water],
      "circle-stroke-opacity": 0.7,
    },
  });

  let beams: Beam[] = [];
  let frame = 0;

  function draw() {
    const now = performance.now();
    beams = beams.filter((b) => now - b.t0 < RUN_MS + FADE_MS);
    const features: GeoJSON.Feature[] = [];
    for (const b of beams) {
      const k = Math.min(1, (now - b.t0) / RUN_MS);
      const at: [number, number] = [b.from[0] + (b.to[0] - b.from[0]) * k, b.from[1] + (b.to[1] - b.from[1]) * k];
      const a = k < 1 ? 1 : Math.max(0, 1 - (now - b.t0 - RUN_MS) / FADE_MS);
      features.push({ type: "Feature", properties: { side: b.side, a }, geometry: { type: "LineString", coordinates: [b.from, at] } });
      if (k < 1) features.push({ type: "Feature", properties: { side: b.side }, geometry: { type: "Point", coordinates: at } });
    }
    (map.getSource("beam") as maplibregl.GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features });
    frame = beams.length ? requestAnimationFrame(draw) : 0;
  }

  /** A permanent source glows for a moment. */
  function glow(ll: [number, number], side: Faction) {
    // MapLibre places the marker by its transform: the animation goes on a child.
    const el = document.createElement("div");
    el.innerHTML = `<div class="link-glow"></div>`;
    el.style.setProperty("--c", c[side]);
    const m = new maplibregl.Marker({ element: el }).setLngLat(ll).addTo(map);
    setTimeout(() => m.remove(), 2600);
  }

  /** The claim reached from the source at `from` to `to`. */
  function beam(from: [number, number], to: [number, number], side: Faction) {
    if (beams.length >= MAX) return;
    glow(from, side);
    beams.push({ from, to, side, t0: performance.now() });
    if (!frame) frame = requestAnimationFrame(draw);
  }

  if (params.has("sim") || params.has("video")) Object.assign(window, { __beam: beam });
  return { beam, glow };
}
