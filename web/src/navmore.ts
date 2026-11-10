// The less used map buttons (sound, radar, clouds, mood, time-lapse, whole
// city) fold away behind "…" and open as a labelled list beside it, so
// nothing spreads across the map. Upwards if there is room, else downwards.
// Dragging or tapping the map folds them in again.
import type * as maplibregl from "maplibre-gl";
import { $ } from "./page";

export function setupNavMore(map: maplibregl.Map) {
  const nav = $("nav");
  const btn = $("nav-more");
  const drawer = $("nav-drawer");
  const set = (open: boolean) => {
    nav.classList.toggle("open", open);
    btn.setAttribute("aria-expanded", String(open));
    if (!open) return;
    drawer.classList.remove("down");
    if (drawer.getBoundingClientRect().top < 8) drawer.classList.add("down");
  };
  btn.onclick = () => set(!nav.classList.contains("open"));
  map.on("dragstart", () => set(false));
  map.on("click", () => set(false));
}
