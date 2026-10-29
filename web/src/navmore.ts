// The less used map buttons (sound, radar, clouds, time-lapse, whole city)
// fold away behind "…", so the column on the right fits above the claim
// buttons on a phone. Dragging the map folds them in again.
import type * as maplibregl from "maplibre-gl";
import { $ } from "./page";

export function setupNavMore(map: maplibregl.Map) {
  const nav = $("nav");
  const btn = $("nav-more");
  const set = (open: boolean) => {
    nav.classList.toggle("open", open);
    btn.setAttribute("aria-expanded", String(open));
  };
  btn.onclick = () => set(!nav.classList.contains("open"));
  map.on("dragstart", () => set(false));
}
