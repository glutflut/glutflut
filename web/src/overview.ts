// Territory per tile while zoomed out (web.rs `Overview`): the server pushes
// all of it once, then the tiles that changed (every few seconds).
import * as maplibregl from "maplibre-gl";
import type { TileLayer } from "./tiles";
import { COLORS } from "./geo";
import { $ } from "./page";

export function setupOverview(map: maplibregl.Map, tiles: TileLayer, send: (on: boolean) => void) {
  const overview = new Map<string, number[]>();
  let overviewOn = false;
  function rows(rows: number[][], full: boolean) {
    if (full) overview.clear();
    for (const r of rows) overview.set(`${r[0]},${r[1]}`, r);
    if (overviewOn) draw();
  }
  function draw() {
    // Tiles with their streets shown need no square.
    const rows = [...overview.values()].filter(([x, y]) => !tiles.holds(x, y));
    // Colour of the leading side, stronger the more of the tile is taken.
    const data = tiles.polygons(rows, ([, , f, w, fr, n]) => {
      const lead = Math.max(f, w, fr);
      const c = lead === 0 ? COLORS.neutral : lead === f ? COLORS.fire : lead === w ? COLORS.water : COLORS.front;
      return { c, a: 0.15 + 0.55 * ((f + w + fr) / Math.max(1, n)) };
    });
    (map.getSource("overview") as maplibregl.GeoJSONSource).setData(data);
  }
  function show() {
    const on = tiles.tooMany;
    map.setLayoutProperty("overview", "visibility", on ? "visible" : "none");
    $("zoom-hint").hidden = !on;
    if (on) draw();
    if (on !== overviewOn) send(on);
    overviewOn = on;
  }
  return { rows, show, on: () => overviewOn };
}
