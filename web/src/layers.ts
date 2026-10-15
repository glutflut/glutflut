// The map and its game layers: territory squares for the overview, the claim
// range, streets (with glow and core in the game view) and junctions, all
// coloured by the feature-state the world sets (owner `o`, strength `s`).
import * as maplibregl from "maplibre-gl";
import { basemapStyle, localLabels } from "./basemap";
import { setupNeighbours } from "./neighbours";
import { knownRegions } from "./outside";
import { VIEW_BUDGET, type Meta } from "./tiles";
import { COLORS, CORE, GLOW } from "./geo";
import { addNaturalLayer } from "./natural";

export const empty = (): GeoJSON.FeatureCollection => ({ type: "FeatureCollection", features: [] });

/** Glow opacity: stronger holds glow more. */
function glowOpacity(): maplibregl.ExpressionSpecification {
  const o: maplibregl.ExpressionSpecification = ["coalesce", ["feature-state", "o"], 0];
  const s: maplibregl.ExpressionSpecification = ["min", 1, ["coalesce", ["feature-state", "s"], 0]];
  return ["case", ["==", o, 0], 0, ["+", 0.25, ["*", 0.45, s]]];
}

/** Diagonal stripes for blocks held with players' sources. */
function hatch(): ImageData {
  const n = 16;
  const g = new OffscreenCanvas(n, n).getContext("2d")!;
  g.strokeStyle = "#fff";
  g.lineWidth = 2;
  for (const o of [-n, 0, n]) {
    g.beginPath();
    g.moveTo(o, n);
    g.lineTo(o + n, 0);
    g.stroke();
  }
  return g.getImageData(0, 0, n, n);
}

/** The map, loaded, with every layer the game draws into (`full`: tilt and 3D buildings). */
export async function createMap(meta: Meta, gameView: boolean, full: boolean): Promise<maplibregl.Map> {
  // A whole city doesn't fit: start at the hotspot area (or the centre) close enough for streets.
  const big = meta.nodes > VIEW_BUDGET;
  const map = new maplibregl.Map({
    container: "map",
    style: await basemapStyle(gameView ? "night" : "light", full),
    ...(full ? { pitch: 35, maxPitch: 55 } : {}),
    ...(big
      ? { center: meta.hotspot ?? meta.region.centroid, zoom: 15 }
      : { bounds: meta.region.bounds, fitBoundsOptions: { padding: 20 } }),
    // Far out there is nothing to play; the overview needs the city in view.
    minZoom: 10,
    attributionControl: false,
  });
  // In the game the cards cover the top corners: the credit is in the
  // footer's link line (index.html) instead.
  if (!gameView) map.addControl(new maplibregl.AttributionControl({ compact: true }), "top-right");
  await new Promise<void>((r) => map.on("load", () => r()));

  localLabels(map);

  const OWNER: maplibregl.ExpressionSpecification = ["coalesce", ["feature-state", "o"], 0];
  const STRENGTH: maplibregl.ExpressionSpecification = ["min", 1, ["coalesce", ["feature-state", "s"], 0]];
  const byOwner = (fallback: string, c: { fire: string; water: string; front: string } = gameView ? GLOW : COLORS) =>
    ["match", OWNER, 1, c.fire, 2, c.water, 3, c.front, fallback] as maplibregl.ExpressionSpecification;

  // Zoomed out over a big city: territory per tile instead of streets.
  map.addSource("overview", { type: "geojson", data: empty() });
  map.addLayer({
    id: "overview",
    type: "fill",
    source: "overview",
    layout: { visibility: "none" },
    paint: { "fill-color": ["get", "c"], "fill-opacity": ["get", "a"], "fill-outline-color": "rgba(255,255,255,0.4)" },
  });
  setupNeighbours(map, meta, knownRegions(), COLORS, gameView);
  // Blocks held by one side: a fill under the streets (`[area]`), denser the
  // stronger the border.
  map.addSource("blocks", { type: "geojson", data: empty() });
  map.addLayer({
    id: "blocks",
    type: "fill",
    source: "blocks",
    paint: {
      "fill-color": byOwner("rgba(0,0,0,0)", gameView ? GLOW : COLORS),
      "fill-opacity": [
        "case",
        ["==", OWNER, 0],
        0,
        ["*", gameView ? 0.34 : 0.3, ["+", 0.3, ["*", 0.7, STRENGTH]]],
      ],
    },
  });
  // Held with players' sources: light stripes over the fill (yours brighter).
  map.addImage("hatch", hatch(), { pixelRatio: 2 });
  map.addLayer({
    id: "blocks-players",
    type: "fill",
    source: "blocks",
    filter: [">", ["get", "p"], 0],
    paint: {
      "fill-pattern": "hatch",
      "fill-opacity": ["case", ["==", OWNER, 0], 0, ["==", ["get", "p"], 2], gameView ? 0.3 : 0.4, gameView ? 0.13 : 0.18],
    },
  });
  map.addSource("range", { type: "geojson", data: empty() });
  map.addLayer({ id: "range", type: "fill", source: "range", paint: { "fill-color": COLORS.neutral, "fill-opacity": 0.12 } });
  map.addLayer({
    id: "range-line",
    type: "line",
    source: "range",
    paint: { "line-color": COLORS.neutral, "line-width": 1.5, "line-dasharray": [2, 2] },
  });
  map.addSource("edges", { type: "geojson", data: empty() });
  const round = { "line-cap": "round", "line-join": "round" } as const;
  if (gameView) {
    // Soft glow under owned streets. Static: animating a feature-state paint
    // property recomputes every street on the CPU (measured: 13x idle cost).
    map.addLayer({
      id: "edges-glow",
      type: "line",
      source: "edges",
      layout: round,
      paint: {
        "line-color": byOwner("rgba(0,0,0,0)"),
        "line-width": ["interpolate", ["exponential", 1.6], ["zoom"], 13, 3, 16, 10, 19, 24],
        "line-blur": ["interpolate", ["exponential", 1.6], ["zoom"], 13, 2, 16, 7, 19, 16],
        "line-opacity": glowOpacity(),
      },
    });
  }
  map.addLayer({
    id: "edges",
    type: "line",
    source: "edges",
    layout: round,
    paint: gameView
      ? {
          "line-color": byOwner(GLOW.neutral),
          "line-width": ["interpolate", ["linear"], ["zoom"], 14, 1.5, 18, 5],
          "line-opacity": ["case", ["==", OWNER, 0], 0.8, ["+", 0.55, ["*", 0.45, STRENGTH]]],
        }
      : {
          "line-color": byOwner(COLORS.neutral),
          "line-width": ["interpolate", ["linear"], ["zoom"], 14, 1.5, 18, 6],
          "line-opacity": 0.9,
        },
  });
  if (gameView) {
    // Hot core: the stronger the hold, the brighter.
    map.addLayer({
      id: "edges-core",
      type: "line",
      source: "edges",
      layout: round,
      paint: {
        "line-color": byOwner("rgba(0,0,0,0)", CORE),
        "line-width": ["interpolate", ["linear"], ["zoom"], 14, 0.4, 18, 1.6],
        "line-opacity": ["case", ["==", OWNER, 0], 0, ["*", 0.9, STRENGTH]],
      },
    });
  }
  map.addSource("nodes", { type: "geojson", data: empty() });
  map.addLayer({
    id: "nodes",
    type: "circle",
    source: "nodes",
    paint: gameView
      ? {
          // Only held junctions, as small sparks when close.
          "circle-color": byOwner("rgba(0,0,0,0)", CORE),
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 15, 1, 18, 3.5],
          "circle-blur": 0.4,
          "circle-opacity": ["interpolate", ["linear"], ["zoom"], 15, 0, 16.5, ["case", ["==", OWNER, 0], 0, 0.9]],
          "circle-pitch-alignment": "map",
        }
      : {
          "circle-color": byOwner(COLORS.neutral),
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 14, 1.5, 18, 6],
          "circle-stroke-color": "#fff",
          "circle-stroke-width": ["interpolate", ["linear"], ["zoom"], 15, 0, 18, 1.5],
        },
  });
  addNaturalLayer(map, gameView);
  return map;
}
