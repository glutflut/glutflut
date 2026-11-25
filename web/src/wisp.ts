// The will-o'-the-wisp (server `wisp.rs`): a light that wanders the streets.
// White when it belongs to nobody, in a side's colour as it leans to it.
// It glides from junction to junction; a tap says what it is.
import * as maplibregl from "maplibre-gl";
import { GLOW } from "./geo";
import type { WispShown } from "./protocol";

const GLIDE_MS = 4000;

function mix(a: string, b: string, k: number): string {
  const p = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
  const [x, y] = [p(a), p(b)];
  return `rgb(${x.map((v, i) => Math.round(v + (y[i] - v) * k)).join(",")})`;
}

export function setupWisp(map: maplibregl.Map, onTap: () => void) {
  const el = document.createElement("div");
  el.className = "wisp";
  el.innerHTML = `<div class="wisp-core"></div>`;
  el.addEventListener("click", (e) => {
    e.stopPropagation();
    onTap();
  });
  const marker = new maplibregl.Marker({ element: el });
  let at: [number, number] | null = null;
  let anim = 0;

  return {
    /** The server's word; true the first time it appears. */
    set(m: WispShown): boolean {
      const first = at === null;
      const k = Math.min(1, Math.abs(m.lean) / 0.8);
      const c = Math.abs(m.lean) < 0.15 ? "#e9f1ff" : mix("#e9f1ff", m.lean > 0 ? GLOW.fire : GLOW.water, k);
      el.style.setProperty("--c", c);
      const to: [number, number] = [m.lon, m.lat];
      if (first) {
        at = to;
        marker.setLngLat(to).addTo(map);
        return true;
      }
      const from = at!;
      at = to;
      if (from[0] === to[0] && from[1] === to[1]) return false;
      cancelAnimationFrame(anim);
      const t0 = performance.now();
      const step = (t: number) => {
        const u = Math.min(1, (t - t0) / GLIDE_MS);
        const e = u * u * (3 - 2 * u);
        marker.setLngLat([from[0] + (to[0] - from[0]) * e, from[1] + (to[1] - from[1]) * e]);
        if (u < 1) anim = requestAnimationFrame(step);
      };
      anim = requestAnimationFrame(step);
      return false;
    },
  };
}
