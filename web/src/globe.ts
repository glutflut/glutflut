// Zoomed far out the map turns into a globe with every city; from there
// "Wetter-Globus" opens Weathermarble at the same spot, and a link back with
// `#look=lat,lon,zoom` flies in from that height.
import * as maplibregl from "maplibre-gl";
import { t } from "./i18n";
import { $ } from "./page";
import { COLORS } from "./geo";
import type { RegionInfo } from "./region";

/** Below this zoom the globe, above the flat map again (a gap against flicker). */
const GLOBE_IN = 6.5;
const GLOBE_OUT = 7.5;
/** A faint glow at the globe's edge (MapLibre's atmosphere), none on the flat map. */
const sky = (glow: number): maplibregl.SkySpecification => ({
  "sky-color": "transparent",
  "horizon-color": "transparent",
  "fog-color": "transparent",
  "fog-ground-blend": 1,
  "atmosphere-blend": glow,
});
export const WEATHER_GLOBE = "https://www.weathermarble.com/glutflut/";
/** No way home (watching). */
const none = () => false;

/** `#look=lat,lon,zoom` from a link (the globe's way back), once. */
export function lookFromHash(): { center: [number, number]; zoom: number } | null {
  const m = /(?:^#|&)look=(-?[\d.]+),(-?[\d.]+),([\d.]+)/.exec(location.hash);
  if (!m) return null;
  history.replaceState(null, "", location.pathname + location.search);
  return { center: [Number(m[2]), Number(m[1])], zoom: Number(m[3]) };
}

export function setupGlobe(
  map: maplibregl.Map,
  regions: () => RegionInfo[],
  here: string,
  open: (r: RegionInfo) => void,
  /** Playing: back to the player's own position (true when it could). */
  home: () => boolean = none,
) {
  map.setMinZoom(1.5);
  let globe = false;
  const bar = $("globe-bar");
  const link = $("globe-weather") as HTMLAnchorElement;
  const centre = (r: RegionInfo): [number, number] => [(r.bounds[0] + r.bounds[2]) / 2, (r.bounds[1] + r.bounds[3]) / 2];

  // Every city as a dot and a label, up to zoom 10. Labels give way to each
  // other (MapLibre's collision), so zooming into a cluster separates them.
  function cities() {
    if (map.getSource("cities")) return;
    const list = regions().filter((r) => r.listed !== false || r.slug === here);
    map.addSource("cities", {
      type: "geojson",
      data: {
        type: "FeatureCollection",
        features: list.map((r) => ({
          type: "Feature",
          properties: { slug: r.slug, city: r.city, here: r.slug === here },
          geometry: { type: "Point", coordinates: centre(r) },
        })),
      },
    });
    map.addLayer({
      id: "cities-dot",
      type: "circle",
      source: "cities",
      maxzoom: 10,
      paint: {
        "circle-radius": ["case", ["get", "here"], 7, 5],
        "circle-color": "#ffd43b",
        "circle-stroke-width": ["case", ["get", "here"], 2.5, 1],
        "circle-stroke-color": ["case", ["get", "here"], "#ffffff", "#16181d"],
      },
    });
    map.addLayer({
      id: "cities-label",
      type: "symbol",
      source: "cities",
      maxzoom: 10,
      layout: {
        "text-field": ["get", "city"],
        "text-font": ["Noto Sans Medium"],
        "text-size": 13,
        "text-offset": [0, 1.1],
        "text-anchor": "top",
        "symbol-sort-key": ["case", ["get", "here"], 0, 1],
      },
      paint: { "text-color": "#f1f3f5", "text-halo-color": "#16181d", "text-halo-width": 1.6 },
    });
    for (const id of ["cities-dot", "cities-label"]) {
      map.on("click", id, (e) => {
        const slug = e.features?.[0]?.properties?.slug as string | undefined;
        const r = regions().find((x) => x.slug === slug);
        if (!r) return;
        if (r.slug !== here) return open(r);
        map.flyTo({ center: centre(r), zoom: 14, duration: 4000, essential: true });
      });
      map.on("mouseenter", id, () => (map.getCanvas().style.cursor = "pointer"));
      map.on("mouseleave", id, () => (map.getCanvas().style.cursor = ""));
    }
  }

  function look() {
    const c = map.getCenter();
    link.href = `${WEATHER_GLOBE}#look=${c.lat.toFixed(3)},${c.lng.toFixed(3)},${map.getZoom().toFixed(1)}`;
  }

  // The tiles end at 85°: the caps show the background, on the globe in sea colour.
  let land: maplibregl.PropertyValueSpecification<string> | undefined;
  function polar(on: boolean) {
    if (!map.getLayer("background") || !map.getLayer("water")) return;
    const sea = map.getPaintProperty("water", "fill-color");
    if (typeof sea !== "string") return;
    if (on) land = map.getPaintProperty("background", "background-color") as typeof land;
    if (on || land) map.setPaintProperty("background", "background-color", on ? sea : land);
  }

  // The world ranking (season points over all cities and shards), fetched
  // when the globe comes up, at most every five minutes.
  let worldAt = 0;
  async function world() {
    if (Date.now() - worldAt < 300_000) return;
    worldAt = Date.now();
    try {
      const r = await fetch("/api/world");
      if (!r.ok) return;
      const d = (await r.json()) as { players: { name: string; faction: "fire" | "water"; points: number; cities: number }[] };
      const box = $("globe-world");
      const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
      box.querySelector("ol")!.innerHTML = d.players
        .slice(0, 20)
        .map(
          (p) =>
            `<li><span class="dot" style="background:${COLORS[p.faction]}"></span><span class="name">${esc(p.name)}</span>` +
            `<span class="pts">${p.points.toLocaleString()}</span><span class="cities">${t(p.cities === 1 ? "globe.city.one" : "globe.city.n", { n: p.cities })}</span></li>`,
        )
        .join("");
      box.hidden = d.players.length === 0;
    } catch {
      // Offline or an old server: no list.
    }
  }

  map.on("zoom", () => {
    const z = map.getZoom();
    if (z < 10) cities();
    if (!globe && z < GLOBE_IN) {
      globe = true;
      void world();
      map.setProjection({ type: "globe" });
      map.setSky(sky(0.6));
      polar(true);
      bar.hidden = false;
      $("globe-back").textContent = t(home === none ? "globe.back" : "globe.me");
      document.body.classList.add("globe");
    } else if (globe && z > GLOBE_OUT) {
      globe = false;
      map.setProjection({ type: "mercator" });
      map.setSky(sky(0));
      polar(false);
      bar.hidden = true;
      document.body.classList.remove("globe");
    }
  });
  map.on("moveend", () => globe && look());
  link.textContent = t("globe.weather");
  // Players go back to where they stand, watchers to the city.
  $("globe-back").onclick = () => {
    if (home()) return;
    const r = regions().find((x) => x.slug === here);
    if (r) map.flyTo({ center: centre(r), zoom: 14, duration: 4000, essential: true });
  };
}
