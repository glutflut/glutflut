// The small map after a claim through a link into another region (server
// far.rs `near`): that region's basemap, its junctions around the spot by
// owner, a pulse where it fell. It fades after a few seconds; a tap opens
// that region. Sim and `?video` expose it as `window.__inset` (layout checks).
import * as maplibregl from "maplibre-gl";
import { basemapStyle } from "./basemap";
import { COLORS, GLOW } from "./geo";
import { $, params } from "./page";
import { listRegions, openRegion } from "./region";

/** How long the small map stays. */
const SHOW_MS = 7000;

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
      zoom: 16,
      interactive: false,
      attributionControl: false,
    });
    await new Promise((r) => m.once("load", r));
    m.addSource("near", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
    m.addLayer({
      id: "near",
      type: "circle",
      source: "near",
      paint: {
        "circle-radius": ["case", ["get", "spot"], 7, 4],
        "circle-color": ["match", ["get", "o"], 1, c.fire, 2, c.water, c.neutral],
        "circle-stroke-width": ["case", ["get", "spot"], 2, 0],
        "circle-stroke-color": "#fff",
      },
    });
    map = m;
    return m;
  }

  /** Show the place in `region` (`name`) where a claim through a link fell. */
  async function show(region: string, name: string, at: [number, number], near: [number, number, number][]) {
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
    (m.getSource("near") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: near.map(([lon, lat, o]) => ({
        type: "Feature",
        properties: { o, spot: spot(lon, lat) },
        geometry: { type: "Point", coordinates: [lon, lat] },
      })),
    });
    m.resize();
    m.jumpTo({ center: at, zoom: 16 });
  }

  if (params.has("sim") || params.has("video")) Object.assign(window, { __inset: show });
  return { show };
}
