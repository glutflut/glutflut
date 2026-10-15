// Natural sources (tiles.rs `s`): a small flame, drop or spring marker just
// above the junction, so a player's ring on the same junction leaves it visible.
import type * as maplibregl from "maplibre-gl";
import { COLORS, GLOW } from "./geo";

const PX = 2; // drawn at twice the size for sharp icons
const W = 18;
const H = 22;

function icon(kind: 0 | 1 | 2, gameView: boolean): ImageData {
  const c = new OffscreenCanvas(W * PX, H * PX);
  const g = c.getContext("2d")!;
  g.scale(PX, PX);
  const col = kind === 0 ? (gameView ? GLOW.fire : COLORS.fire) : gameView ? GLOW.water : COLORS.water;
  const cx = W / 2;
  const shape = new Path2D();
  if (kind === 0) {
    // Flame: a tip curving up from a round base.
    shape.moveTo(cx, 2);
    shape.bezierCurveTo(cx + 3, 7, cx + 7, 9, cx + 6, 14);
    shape.bezierCurveTo(cx + 5.5, 18.5, cx + 2.5, 20, cx, 20);
    shape.bezierCurveTo(cx - 2.5, 20, cx - 5.5, 18.5, cx - 6, 14);
    shape.bezierCurveTo(cx - 6.5, 10.5, cx - 3.5, 9, cx - 2.5, 5.5);
    shape.bezierCurveTo(cx - 1, 8, cx + 0.5, 6, cx, 2);
  } else {
    // Drop (spring: smaller, with a ring for the basin).
    const s = kind === 2 ? 0.7 : 1;
    const top = kind === 2 ? 5 : 2;
    const r = 6 * s;
    const cy = 20 - r - (kind === 2 ? 3 : 0);
    shape.moveTo(cx, top);
    shape.bezierCurveTo(cx + 2 * s, top + 5 * s, cx + r, cy - 2 * s, cx + r, cy);
    shape.arc(cx, cy, r, 0, Math.PI);
    shape.bezierCurveTo(cx - r, cy - 2 * s, cx - 2 * s, top + 5 * s, cx, top);
  }
  g.lineJoin = "round";
  g.lineWidth = 2.4;
  g.strokeStyle = gameView ? "rgba(0,0,0,0.75)" : "#fff";
  g.stroke(shape);
  g.fillStyle = col;
  g.fill(shape);
  if (kind === 2) {
    g.beginPath();
    g.ellipse(cx, 19.5, 6.5, 2, 0, 0, 2 * Math.PI);
    g.lineWidth = 1.4;
    g.strokeStyle = col;
    g.stroke();
  }
  // A light core: reads as glowing in the dark view.
  g.beginPath();
  g.ellipse(cx, kind === 0 ? 15 : kind === 2 ? 13.5 : 14.5, kind === 2 ? 1.5 : 2, kind === 2 ? 2 : 2.8, 0, 0, 2 * Math.PI);
  g.fillStyle = "rgba(255,255,255,0.75)";
  g.fill();
  return g.getImageData(0, 0, W * PX, H * PX);
}

export function addNaturalLayer(map: maplibregl.Map, gameView: boolean) {
  for (const k of [0, 1, 2] as const) map.addImage(`nat-${k}`, icon(k, gameView), { pixelRatio: PX });
  map.addSource("natural", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({
    id: "natural",
    type: "symbol",
    source: "natural",
    minzoom: 14,
    layout: {
      "icon-image": ["concat", "nat-", ["to-string", ["get", "k"]]],
      "icon-anchor": "bottom",
      "icon-offset": [0, -3],
      "icon-size": ["interpolate", ["linear"], ["zoom"], 14, 0.55, 16, 0.85, 18, 1.15],
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
      "icon-pitch-alignment": "viewport",
    },
    paint: { "icon-opacity": ["interpolate", ["linear"], ["zoom"], 14, 0, 14.8, 0.95] },
  });
}
