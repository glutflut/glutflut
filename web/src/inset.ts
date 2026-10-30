// The small map after a claim through a link into another region (server
// far.rs `near`): that region's basemap, its streets around the spot glowing
// by owner like the main map, a pulse where it fell. It fades after a few seconds; a tap opens
// that region. Sim and `?video` expose it as `window.__inset` (layout checks).
import * as maplibregl from "maplibre-gl";
import { basemapStyle } from "./basemap";
import { COLORS, CORE, GLOW, owner } from "./geo";
import { $, params } from "./page";
import { listRegions, openRegion } from "./region";

/** How long the small map stays. */
const SHOW_MS = 7000;
/** Close enough to see single streets. */
const ZOOM = 16.6;

export function setupInset(gameView: boolean) {
  const box = $("inset");
  const c = gameView ? GLOW : COLORS;
  let map: maplibregl.Map | null = null;
  let mapRegion = "";
  let open = "";
  let hide = 0;
  box.onclick = () => {
    if (!open) return;
    void listRegions().then((rs) => {
      const r = rs.find((x) => x.slug === open);
      if (r) void openRegion(r);
    });
  };

  async function mapFor(region: string): Promise<maplibregl.Map> {
    if (map && mapRegion === region) return map;
    map?.remove();
    mapRegion = region;
    const m = new maplibregl.Map({
      container: "inset-map",
      style: await basemapStyle(gameView ? "night" : "light", false, region),
      center: [0, 0],
      zoom: ZOOM,
      interactive: false,
      attributionControl: false,
    });
    await new Promise((r) => m.once("load", r));
    const o = ["get", "o"] as maplibregl.ExpressionSpecification;
    const s = ["get", "s"] as maplibregl.ExpressionSpecification;
    const byOwner = (fallback: string, cc: { fire: string; water: string; front: string } = c) =>
      ["match", o, 1, cc.fire, 2, cc.water, 3, cc.front, fallback] as maplibregl.ExpressionSpecification;
    const round = { "line-cap": "round", "line-join": "round" } as const;
    m.addSource("streets", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
    m.addLayer({
      id: "glow",
      type: "line",
      source: "streets",
      layout: round,
      paint: { "line-color": byOwner("rgba(0,0,0,0)"), "line-width": 16, "line-blur": 10, "line-opacity": 0.7 },
    });
    m.addLayer({
      id: "streets",
      type: "line",
      source: "streets",
      layout: round,
      paint: {
        "line-color": byOwner(c.neutral),
        "line-width": 4.5,
        "line-opacity": ["case", ["==", o, 0], 0.7, ["+", 0.6, ["*", 0.4, s]]],
      },
    });
    m.addLayer({
      id: "core",
      type: "line",
      source: "streets",
      layout: round,
      paint: { "line-color": byOwner("rgba(0,0,0,0)", CORE), "line-width": 1.4, "line-opacity": ["case", ["==", o, 0], 0, ["*", 0.9, s]] },
    });
    m.addSource("near", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
    m.addLayer({
      id: "near",
      type: "circle",
      source: "near",
      filter: ["any", ["get", "spot"], [">", o, 0]],
      paint: {
        "circle-radius": ["case", ["get", "spot"], 7, 3.5],
        "circle-color": ["case", ["get", "spot"], byOwner(c.neutral), byOwner(c.neutral, CORE)],
        "circle-stroke-width": ["case", ["get", "spot"], 2, 0],
        "circle-stroke-color": "#fff",
      },
    });
    map = m;
    return m;
  }

  /** Show the place in `region` (`name`) where a claim through a link fell. */
  async function show(
    region: string,
    name: string,
    at: [number, number],
    near: [number, number, number, number][],
    streets: [number, number][] = [],
  ) {
    open = region;
    $("inset-name").textContent = name;
    box.classList.remove("out");
    box.hidden = false;
    clearTimeout(hide);
    hide = window.setTimeout(() => {
      box.classList.add("out");
      hide = window.setTimeout(() => (box.hidden = true), 600);
    }, SHOW_MS);
    const m = await mapFor(region);
    const spot = (lon: number, lat: number) => Math.abs(lon - at[0]) < 1e-7 && Math.abs(lat - at[1]) < 1e-7;
    const props = ([, , f, w]: number[]) => ({ o: owner(f, w), s: Math.min(1, Math.max(f, w)) });
    (m.getSource("near") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: near.map((n) => ({
        type: "Feature",
        properties: { ...props(n), spot: spot(n[0], n[1]) },
        geometry: { type: "Point", coordinates: [n[0], n[1]] },
      })),
    });
    // A street takes the mean of its ends, as on the main map (world.ts).
    (m.getSource("streets") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: streets
        .filter(([a, b]) => near[a] && near[b])
        .map(([a, b]) => ({
          type: "Feature",
          properties: props([0, 0, (near[a][2] + near[b][2]) / 2, (near[a][3] + near[b][3]) / 2]),
          geometry: { type: "LineString", coordinates: [near[a].slice(0, 2), near[b].slice(0, 2)] },
        })),
    });
    m.resize();
    m.jumpTo({ center: at, zoom: ZOOM });
  }

  if (params.has("sim") || params.has("video")) Object.assign(window, { __inset: show });
  return { show };
}
