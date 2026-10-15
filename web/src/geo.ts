// Element thresholds and colours, small geometry on lon/lat pairs.

// Same thresholds as shared/src/elements.rs.
export const ELEMENT_ACTIVE = 0.15;
export const ELEMENT_MARGIN = 0.05;
export const CLAIM_RADIUS_M = 80;
// Same values as colors.css (MapLibre paint needs literals).
export const COLORS = { fire: "#e8590c", water: "#1c7ed6", front: "#ae3ec9", neutral: "#868e96" };
// Game view: brighter element colours, a hot core, neutral streets a light grey
// above the darker basemap roads (no street of the game).
export const GLOW = { fire: "#ff7a1a", water: "#2f9bff", front: "#d05cf0", neutral: "#6c7889" };
export const CORE = { fire: "#ffe0b3", water: "#c9e8ff", front: "#f5d0ff" };

// Owner codes used as feature-state: 0 neutral, 1 fire, 2 water, 3 front.
export function owner(f: number, w: number): number {
  if (Math.max(f, w) < ELEMENT_ACTIVE) return 0;
  if (Math.abs(f - w) < ELEMENT_MARGIN) return 3;
  return f > w ? 1 : 2;
}

/** Metres east and north of `origin` (flat earth; fine over a few km). The
 *  web's one local projection, like project_enu on the Rust side. */
export function enu(p: [number, number], origin: [number, number]): [number, number] {
  const k = Math.cos((origin[1] * Math.PI) / 180);
  return [(p[0] - origin[0]) * 111_320 * k, (p[1] - origin[1]) * 110_540];
}

/** Metres -> degrees of lon and lat at a latitude (inverse of `enu`). */
export function degPerM(lat: number): [number, number] {
  return [1 / (111_320 * Math.cos((lat * Math.PI) / 180)), 1 / 110_540];
}

const ARROWS = ["↑", "↗", "→", "↘", "↓", "↙", "←", "↖"];

/** Arrow from `a` towards `b` (eight directions, north up). */
export function arrow(a: [number, number], b: [number, number]): string {
  const k = Math.cos((a[1] * Math.PI) / 180);
  const deg = (Math.atan2((b[0] - a[0]) * k, b[1] - a[1]) * 180) / Math.PI;
  return ARROWS[Math.round(((deg + 360) % 360) / 45) % 8];
}

export function distM(a: [number, number], b: [number, number]): number {
  return Math.hypot(...enu(b, a));
}

export function circle(c: [number, number], r: number): GeoJSON.Feature<GeoJSON.Polygon> {
  const [kx, ky] = degPerM(c[1]);
  const ring: [number, number][] = [];
  for (let i = 0; i <= 48; i++) {
    const a = (i / 48) * 2 * Math.PI;
    ring.push([c[0] + r * Math.cos(a) * kx, c[1] + r * Math.sin(a) * ky]);
  }
  return { type: "Feature", properties: {}, geometry: { type: "Polygon", coordinates: [ring] } };
}
