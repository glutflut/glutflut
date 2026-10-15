// Points rising from where they were earned: claims and ratings at their
// junction, rings, sieges and holding (the minute's street and area points,
// web.rs `Score`) above the player.
import * as maplibregl from "maplibre-gl";
import type { Faction } from "./net";

export function setupFloaters(map: maplibregl.Map, me: () => [number, number] | null, faction: () => Faction | null) {
  let last: number | null = null;
  // Points already shown since the last score update.
  let shown = 0;
  let timer = 0;

  function at(ll: [number, number] | null | undefined, n: number, cls = "") {
    if (!ll || n <= 0) return;
    // The marker moves its element by transform; the text inside rises.
    const el = document.createElement("div");
    el.className = "float-at";
    const txt = el.appendChild(document.createElement("span"));
    txt.className = `float-pts ${faction() ?? ""} ${cls}`;
    txt.textContent = `+${n}`;
    const m = new maplibregl.Marker({ element: el, anchor: "bottom" }).setLngLat(ll).addTo(map);
    txt.addEventListener("animationend", () => m.remove(), { once: true });
  }
  return {
    /** An event with its points: shown at `ll` (default: the player). */
    earned(n: number, ll?: [number, number] | null, cls = "") {
      if (n <= 0) return;
      shown += n;
      at(ll ?? me(), n, cls);
    },
    /** New day total: what no event explained is holding points. Events of
     * the same tick may arrive just after, hence the wait. */
    score(today: number) {
      clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (last !== null && today > last) at(me(), today - last - shown, "hold");
        if (last === null || today >= last) shown = 0;
        last = today;
      }, 600);
    },
  };
}
