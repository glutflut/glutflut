// Neighbour regions in the map: outline and name always; their territory pale
// and read-only (per tile) only while the neighbour runs anyway, so looking
// never wakes it (server: `/api/overview?peek=1`). No play across the border.
import type * as maplibregl from "maplibre-gl";
import type { RegionInfo } from "./region";
import { xToLon, yToLat, type Meta } from "./tiles";

/** Regions whose bounds come this close (degrees) count as neighbours. */
const NEAR_DEG = 0.25;
const EVERY_MS = 20_000;

type Colors = { fire: string; water: string; front: string };

function near(a: Meta["region"]["bounds"], b: RegionInfo["bounds"], pad: number) {
  return b[0] <= a[2] + pad && b[2] >= a[0] - pad && b[1] <= a[3] + pad && b[3] >= a[1] - pad;
}

export function setupNeighbours(map: maplibregl.Map, meta: Meta, list: RegionInfo[], colors: Colors, dark: boolean) {
  const nbs = list.filter((r) => r.slug !== meta.region.slug && !r.url && near(meta.region.bounds, r.bounds, NEAR_DEG));
  if (!nbs.length) return;
  const own = new Set(meta.tiles.index.map(([x, y]) => `${x},${y}`));
  const ink = dark ? "#c8ccd0" : "#5c6166";

  map.addSource("nb-terr", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer(
    { id: "nb-terr", type: "fill", source: "nb-terr", paint: { "fill-color": ["get", "c"], "fill-opacity": ["get", "a"] } },
    "overview",
  );
  map.addSource("nb-outline", {
    type: "geojson",
    data: {
      type: "FeatureCollection",
      features: nbs.map((r) => ({ type: "Feature", properties: { name: r.city }, geometry: r.outline })),
    },
  });
  map.addLayer(
    {
      id: "nb-line",
      type: "line",
      source: "nb-outline",
      // Soft band, unlike the basemap's dashed borders.
      paint: { "line-color": ink, "line-width": 3, "line-opacity": 0.35 },
    },
    "overview",
  );
  map.addLayer(
    {
      id: "nb-label",
      type: "symbol",
      source: "nb-outline",
      layout: {
        "symbol-placement": "line",
        "symbol-spacing": 200,
        // Borders are jagged: let the name bend.
        "text-max-angle": 80,
        "text-field": ["get", "name"],
        "text-font": ["Noto Sans Medium"],
        "text-size": 12,
        "text-offset": [0, 1],
      },
      paint: { "text-color": ink, "text-halo-color": dark ? "#000" : "#fff", "text-halo-width": 1.2 },
    },
    "overview",
  );

  // Rows per neighbour: [x, y, fire, water, front, all].
  const rows = new Map<string, number[][]>();
  function draw() {
    const features: GeoJSON.Feature[] = [];
    for (const rs of rows.values())
      for (const [x, y, f, w, fr, n] of rs) {
        // Owned tiles only, and none the own region shares.
        const lead = Math.max(f, w, fr);
        if (!lead || own.has(`${x},${y}`)) continue;
        const z = meta.tiles.z;
        const [wl, el, nl, sl] = [xToLon(x, z), xToLon(x + 1, z), yToLat(y, z), yToLat(y + 1, z)];
        features.push({
          type: "Feature",
          properties: {
            c: lead === f ? colors.fire : lead === w ? colors.water : colors.front,
            a: 0.05 + 0.15 * ((f + w + fr) / Math.max(1, n)),
          },
          geometry: { type: "Polygon", coordinates: [[[wl, sl], [el, sl], [el, nl], [wl, nl], [wl, sl]]] },
        });
      }
    (map.getSource("nb-terr") as maplibregl.GeoJSONSource).setData({ type: "FeatureCollection", features });
  }

  let busy = false;
  async function poll() {
    if (busy || document.hidden) return;
    busy = true;
    try {
      const v = map.getBounds();
      const view: Meta["region"]["bounds"] = [v.getWest(), v.getSouth(), v.getEast(), v.getNorth()];
      let changed = false;
      for (const r of nbs) {
        if (!near(view, r.bounds, 0)) continue;
        const res = await fetch(`/api/overview?region=${encodeURIComponent(r.slug)}&peek=1`).catch(() => null);
        if (!res) continue;
        const got: number[][] = res.status === 200 ? ((await res.json().catch(() => null))?.t ?? []) : [];
        if (got.length || rows.has(r.slug)) changed = true;
        if (got.length) rows.set(r.slug, got);
        else rows.delete(r.slug);
      }
      if (changed) draw();
    } finally {
      busy = false;
    }
  }
  map.on("moveend", poll);
  document.addEventListener("visibilitychange", poll);
  setInterval(poll, EVERY_MS);
  poll();
}
