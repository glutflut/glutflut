// Hotspots (web.rs `Hotspots`): pulsing markers, a hint with the distance to
// the nearest, and the "hot" button that flies there.
import * as maplibregl from "maplibre-gl";
import { t } from "./i18n";
import { distM } from "./geo";
import { $ } from "./page";

export function setupHotspots(map: maplibregl.Map, me: () => [number, number] | null, setFollow: (on: boolean) => void) {
  let hotspotMarkers: maplibregl.Marker[] = [];
  let hotspotAt: [number, number][] = [];
  let radiusM = 40; // until the welcome (`hotspot_m`)
  function show(items: { ll: [number, number] }[]) {
    for (const m of hotspotMarkers) m.remove();
    hotspotAt = items.map((h) => h.ll);
    $("nav-hot").hidden = !hotspotAt.length;
    hotspotMarkers = hotspotAt.map((ll) =>
      new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "hotspot" }) })
        .setLngLat(ll)
        .addTo(map),
    );
    hint();
  }
  function hint() {
    const el = $("hotspot-hint");
    const p = me();
    if (!p || !hotspotAt.length) return void (el.hidden = true);
    const d = Math.min(...hotspotAt.map((ll) => distM(p, ll)));
    // Whole-city play area: a hotspot across town is no invitation.
    if (d > 1500) return void (el.hidden = true);
    el.textContent = d <= radiusM ? t("hotspot.here") : t("hotspot.dist", { m: Math.round(d) });
    el.hidden = false;
  }
  $("nav-hot").onclick = () => {
    if (!hotspotAt.length) return;
    const from = me() ?? (map.getCenter().toArray() as [number, number]);
    const near = hotspotAt.reduce((a, b) => (distM(from, a) <= distM(from, b) ? a : b));
    setFollow(false);
    map.flyTo({ center: near, zoom: Math.max(map.getZoom(), 16.5) });
  };
  function setRadius(m: number) {
    radiusM = m;
    hint();
  }
  return { show, hint, setRadius, at: () => hotspotAt };
}
