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
  /** The permanent source it came through: the hexagon of the main map (sources.ts). */
  const srcEl = document.createElement("div");
  srcEl.className = "inset-source";
  let srcMarker: maplibregl.Marker | null = null;
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
    srcMarker = null;
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
    // The link: from the permanent source it came through to the claim.
    m.addSource("link", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
    m.addLayer({
      id: "link-glow",
      type: "line",
      source: "link",
      filter: ["==", ["geometry-type"], "LineString"],
      layout: round,
      paint: { "line-color": byOwner(c.neutral), "line-width": 10, "line-blur": 6, "line-opacity": 0.6 },
    });
    m.addLayer({
      id: "link",
      type: "line",
      source: "link",
      filter: ["==", ["geometry-type"], "LineString"],
      layout: round,
      paint: { "line-color": "#fff", "line-width": 2, "line-dasharray": [1.5, 1.5] },
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
    from: [number, number] | null = null,
  ) {
    open = region;
    $("inset-name").textContent = name;
    box.classList.remove("out");
    box.hidden = false;
    // The pulse on the claim runs a few times per show.
    const pulse = box.querySelector<HTMLElement>(".pulse");
    if (pulse) {
      pulse.style.animation = "none";
      void pulse.offsetWidth;
      pulse.style.animation = "";
    }
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
    const sp = near.find((n) => spot(n[0], n[1]));
    const side = owner(sp?.[2] ?? 0, sp?.[3] ?? 0);
    (m.getSource("link") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: from ? [{ type: "Feature", properties: { o: side }, geometry: { type: "LineString", coordinates: [from, at] } }] : [],
    });
    const color = side === 2 ? c.water : c.fire;
    srcEl.innerHTML =
      `<svg viewBox="0 0 40 40" width="26" height="26">` +
      `<path d="M20 6 L32 13 L32 27 L20 34 L8 27 L8 13 Z" fill="${color}" stroke="#fff" stroke-width="3"/></svg>`;
    srcEl.hidden = !from;
    if (from) (srcMarker ??= new maplibregl.Marker({ element: srcEl }).setLngLat(from).addTo(m)).setLngLat(from);
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
    // The claim in the middle (the pulse), the source it came from in view.
    let zoom = ZOOM;
    if (from) {
      const d: [number, number] = [Math.abs(from[0] - at[0]), Math.abs(from[1] - at[1])];
      const fit = m.cameraForBounds(
        [
          [at[0] - d[0], at[1] - d[1]],
          [at[0] + d[0], at[1] + d[1]],
        ],
        { padding: 24 },
      );
      if (fit?.zoom !== undefined) zoom = Math.min(ZOOM, fit.zoom);
    }
    m.jumpTo({ center: at, zoom });
  }

  if (params.has("sim") || params.has("video")) Object.assign(window, { __inset: show });
  return { show };
}
