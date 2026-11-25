// Fog at the front (server `fog.rs`): steam over junctions where fire and
// water have clashed for long. It covers the streets there, except within
// `see_m` of the player: to see what's going on, go there.
import type * as maplibregl from "maplibre-gl";
import { distM } from "./geo";

/** Radius of the fog around each junction (m). */
const PUFF_M = 40;

export function setupFog(map: maplibregl.Map, nodePos: Map<number, [number, number]>, me: () => [number, number] | null, lat: number) {
  let ids: number[] = [];
  let seeM = 50;
  let key = "";
  const empty = (): GeoJSON.FeatureCollection => ({ type: "FeatureCollection", features: [] });
  map.addSource("fog", { type: "geojson", data: empty() });
  // Metres to pixels at zoom z (512 px tiles); doubles with each zoom step.
  const px = (z: number) => PUFF_M / ((78_271.5 * Math.cos((lat * Math.PI) / 180)) / 2 ** z);
  map.addLayer({
    id: "fog",
    type: "circle",
    source: "fog",
    paint: {
      "circle-color": "#d9dee6",
      "circle-opacity": 0.8,
      "circle-blur": 0.9,
      "circle-radius": ["interpolate", ["exponential", 2], ["zoom"], 10, px(10), 22, px(22)],
      "circle-pitch-alignment": "map",
    },
  });

  function draw() {
    let p: [number, number] | null = null;
    try {
      p = me();
    } catch {
      // The player isn't set up yet (a message before the app is ready).
    }
    const shown = ids.filter((id) => nodePos.has(id));
    // Redraw only when something visible changed (position by 5 m steps).
    const k = `${shown.length}:${shown[0] ?? ""}:${shown.at(-1) ?? ""}:${p ? `${Math.round(p[0] * 2e4)},${Math.round(p[1] * 2e4)}` : ""}`;
    if (k === key) return;
    key = k;
    const features: GeoJSON.Feature[] = [];
    for (const id of shown) {
      const c = nodePos.get(id)!;
      if (p && distM(p, c) < seeM + PUFF_M * 0.5) continue;
      features.push({ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: c } });
    }
    (map.getSource("fog") as maplibregl.GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features });
  }
  setInterval(() => ids.length && draw(), 2000);

  return {
    /** The server's fogged junctions; true if fog is new (none before). */
    set(next: number[], see: number): boolean {
      const fresh = !ids.length && next.length > 0;
      ids = next;
      seeM = see;
      key = "";
      draw();
      return fresh;
    },
  };
}
