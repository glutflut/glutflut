// The dreaming city (server `dream.rs`): at night the map shows the city as it
// dreams. Hazy and violet; some streets fade away, a few appear that don't
// exist (the same ones for everyone that night). `?dream` shows it any time.
import type * as maplibregl from "maplibre-gl";
import { distM } from "./geo";
import type { TileLayer } from "./tiles";

/** Share of streets that fade, of junctions that grow a street that isn't there. */
const GONE = 0.07;
const PHANTOM = 0.025;

/** 0..1 from an id and the night (stable for everyone). */
function hash(id: number, seed: number): number {
  let h = (Math.imul(id | 0, 2654435761) ^ Math.imul(seed, 1597334677)) >>> 0;
  h = Math.imul(h ^ (h >>> 15), 2246822519) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 3266489917) >>> 0;
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

/** The night's number: days since 1970 of the evening it began (local). */
function nightSeed(): number {
  const d = new Date(Date.now() - 12 * 3600_000);
  return Math.floor(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()) / 86_400_000);
}

export function setupDream(map: maplibregl.Map, tiles: TileLayer) {
  const preview = new URLSearchParams(location.search).has("dream");
  let on = false;
  const gone = new Set<number>();
  let drawn = -1;
  map.addSource("dream", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({
    id: "dream-glow",
    type: "line",
    source: "dream",
    layout: { "line-cap": "round" },
    paint: { "line-color": "#b197fc", "line-width": ["interpolate", ["linear"], ["zoom"], 14, 3, 18, 12], "line-blur": 6, "line-opacity": 0.45 },
  });
  map.addLayer({
    id: "dream",
    type: "line",
    source: "dream",
    layout: { "line-cap": "round" },
    paint: { "line-color": "#e5dbff", "line-width": ["interpolate", ["linear"], ["zoom"], 14, 0.8, 18, 2.5], "line-dasharray": [1, 2], "line-opacity": 0.85 },
  });

  function paint() {
    const seed = nightSeed();
    for (const id of tiles.edgeById.keys())
      if (!gone.has(id) && hash(id, seed) < GONE) {
        gone.add(id);
        map.setFeatureState({ source: "edges", id }, { gone: true });
      }
    // Phantom streets: from a few junctions to the nearest one 50-160 m off
    // that no street joins. Only when the loaded junctions changed.
    if (tiles.nodePos.size === drawn) return;
    drawn = tiles.nodePos.size;
    const near = (a: number, b: number) =>
      [...(tiles.edgesOf.get(a) ?? [])].some((e) => {
        const x = tiles.edgeById.get(e);
        return x && (x.source === b || x.target === b);
      });
    const features: GeoJSON.Feature[] = [];
    const all = [...tiles.nodePos];
    for (const [id, p] of all) {
      if (hash(id, seed + 7) >= PHANTOM) continue;
      let best: [number, number] | null = null;
      for (const [j, q] of all) {
        if (j === id) continue;
        const d = distM(p, q);
        if (d >= 50 && d <= 160 && (!best || d < best[1]) && !near(id, j)) best = [j, d];
      }
      if (best) features.push({ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: [p, tiles.nodePos.get(best[0])!] } });
    }
    (map.getSource("dream") as maplibregl.GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features });
  }

  function clear() {
    for (const id of gone) map.setFeatureState({ source: "edges", id }, { gone: false });
    gone.clear();
    drawn = -1;
    (map.getSource("dream") as maplibregl.GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features: [] });
  }

  setInterval(() => on && paint(), 3000);

  const api = {
    /** Night or day; true when the dream just began. */
    set(next: boolean): boolean {
      next ||= preview;
      if (next === on) return false;
      on = next;
      document.body.classList.toggle("dreaming", on);
      if (on) paint();
      else clear();
      return on;
    },
  };
  if (preview) api.set(true);
  return api;
}
