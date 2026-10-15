// Basemap: self-hosted Protomaps extract (web/scripts/fetch-basemap.sh) so
// tile requests never leave our server. Falls back to OpenFreeMap in dev
// builds only when the extract is missing; a built app shows no basemap.
import * as maplibregl from "maplibre-gl";
import { Protocol } from "pmtiles";
import { layers, namedFlavor, type Flavor } from "@protomaps/basemaps";
import { region } from "./region";
import { lang } from "./i18n";

const FALLBACK = "https://tiles.openfreemap.org/styles/positron";
let registered = false;

export type BaseFlavor = "light" | "grayscale" | "night";

// Game view: Protomaps dark, bluer and darker, so fire and water glow.
const NIGHT: Flavor = {
  ...namedFlavor("dark"),
  background: "#0b0e13",
  earth: "#10141b",
  water: "#0a1a2b",
  park_a: "#0f1a17",
  park_b: "#0f1a17",
  wood_a: "#0f1714",
  wood_b: "#0f1714",
  pedestrian: "#141922",
  buildings: "#1a202a",
  // Roads stay darker than the game's neutral streets (GLOW.neutral).
  minor_service: "#161b23",
  minor_a: "#191f28",
  minor_b: "#161b23",
  other: "#161b23",
  link: "#191f28",
  major: "#1d2430",
  highway: "#212833",
  roads_label_minor: "#4a5566",
  roads_label_major: "#5b6678",
  city_label: "#7d8899",
  subplace_label: "#556070",
};

/** `grayscale` keeps a data layer the only colour on the map; `night` is the game view. */
/** `forRegion`: a region other than the app's (the editor picks its own). */
export async function basemapStyle(flavor: BaseFlavor = "light", extrude = true, forRegion = ""): Promise<maplibregl.StyleSpecification | string> {
  const base = `${location.origin}/basemap`;
  // One file per region (docs/deploy.md), else the old shared one. A missing
  // file comes back as the app's index.html, so the type decides.
  let file: string | null = null;
  let version = "";
  const slug =
    forRegion || region || ((await fetch("/api/version").then((r) => r.json()).catch(() => ({}))) as { region?: string }).region || "";
  for (const name of [slug && `${slug}.pmtiles`, "region.pmtiles"].filter(Boolean)) {
    const v = await probe(`${base}/${name}`);
    if (v !== null) {
      file = name as string;
      // A regenerated file gets a new URL, past the day-long cache.
      version = v;
      break;
    }
  }
  if (!file) {
    if (import.meta.env.DEV) {
      console.warn("self-hosted basemap missing, using OpenFreeMap (dev only)");
      return FALLBACK;
    }
    // Built app: no map rather than a third party (the privacy page promises
    // that nothing loads from elsewhere). The game layers still draw.
    console.error("self-hosted basemap missing (docs/deploy.md)");
    return {
      version: 8,
      glyphs: `${base}/fonts/{fontstack}/{range}.pbf`,
      sources: {},
      layers: [{ id: "background", type: "background", paint: { "background-color": flavor === "night" ? NIGHT.background! : "#e9ecef" } }],
    };
  }
  if (!registered) {
    maplibregl.addProtocol("pmtiles", new Protocol().tile);
    registered = true;
  }
  const attribution = '<a href="https://protomaps.com">Protomaps</a> © <a href="https://openstreetmap.org/copyright">OpenStreetMap</a>';
  // No POI icons: they compete with the game colours.
  const style = (source: string) =>
    layers(source, flavor === "night" ? NIGHT : namedFlavor(flavor), { lang }).filter((l) => l.id !== "pois");
  const sources: maplibregl.StyleSpecification["sources"] = {
    protomaps: { type: "vector", url: pmtilesUrl(base, file, version), attribution },
  };
  const own = style("protomaps").map((l) =>
    flavor === "night" && extrude && l.id === "buildings" ? extruded(l as maplibregl.FillLayerSpecification) : l,
  );
  // The region's file covers only its bounds, also zoomed out: below WORLD_UNTIL
  // the whole earth (world.pmtiles, docs/deploy.md), when the host has it,
  // and up to WORLD_UNDER beneath the region (its land and water cover it).
  const world = await probe(`${base}/${WORLD}`);
  if (world === null) return { version: 8, glyphs: `${base}/fonts/{fontstack}/{range}.pbf`, sprite: sprite(base, flavor), sources, layers: own };
  sources.world = { type: "vector", url: pmtilesUrl(base, WORLD, world), attribution };
  const far = style("world")
    .filter((l) => "source" in l && (l.minzoom ?? 0) < WORLD_UNTIL)
    .map((l) => ({ ...l, id: `world-${l.id}`, maxzoom: Math.min(l.maxzoom ?? 24, WORLD_UNDER) }) as maplibregl.LayerSpecification);
  const near = own.map((l) => ("source" in l ? { ...l, minzoom: Math.max(l.minzoom ?? 0, WORLD_UNTIL) } : l));
  const [bg, ...rest] = near;
  return {
    version: 8,
    glyphs: `${base}/fonts/{fontstack}/{range}.pbf`,
    sprite: sprite(base, flavor),
    sources,
    layers: bg && !("source" in bg) ? [bg, ...far, ...rest] : [...far, ...near],
  };
}

/** The whole earth up to zoom 6, one file per host. */
const WORLD = "world.pmtiles";
/** Zoom from which the region's own file draws (the world file below). */
const WORLD_UNTIL = 7;
/** Zoom up to which the world file lies beneath (tilted views see past the region). */
const WORLD_UNDER = 11;

const sprite = (base: string, flavor: BaseFlavor) => `${base}/sprites/${flavor === "night" ? "dark" : flavor}`;
const pmtilesUrl = (base: string, file: string, version: string) =>
  `pmtiles://${base}/${file}${version && `?v=${encodeURIComponent(version)}`}`;

/** Last-Modified of a basemap file ("" without), null when the host lacks it. */
async function probe(url: string): Promise<string | null> {
  const r = await fetch(url, { method: "HEAD", cache: "no-cache" }).catch(() => null);
  if (!r?.ok || r.headers.get("content-type")?.includes("text/html")) return null;
  return r.headers.get("last-modified") ?? "";
}

/** Buildings as blocks (heights from the extract), seen when the map is tilted. */
function extruded(l: maplibregl.FillLayerSpecification): maplibregl.FillExtrusionLayerSpecification {
  return {
    id: l.id,
    type: "fill-extrusion",
    source: l.source,
    "source-layer": l["source-layer"],
    filter: l.filter,
    minzoom: 14,
    paint: {
      "fill-extrusion-color": NIGHT.buildings,
      "fill-extrusion-height": ["coalesce", ["get", "height"], 8],
      "fill-extrusion-base": ["coalesce", ["get", "min_height"], 0],
      "fill-extrusion-opacity": 1,
    },
  };
}

/** Labels in the current language on the fallback style (Protomaps handles `lang` itself). */
export function localLabels(map: maplibregl.Map) {
  for (const l of map.getStyle().layers) {
    if (l.type === "symbol" && map.getLayoutProperty(l.id, "text-field") && l.source !== "protomaps") {
      map.setLayoutProperty(l.id, "text-field", ["coalesce", ["get", `name:${lang}`], ["get", "name"]]);
    }
  }
}
