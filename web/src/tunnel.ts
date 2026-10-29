// Tunnels (server: web.rs `Tunnels`): rarely a claim opens a tunnel to a far
// junction the other side holds where its element gathers (fire on a hilltop,
// water in a sink). For a short while the player can claim it from where
// they stand; they don't move.
import * as maplibregl from "maplibre-gl";
import { lang, t } from "./i18n";
import type { Faction, ServerMsg } from "./net";
import { arrow } from "./geo";
import { $ } from "./page";

export type TunnelMsg = Extract<ServerMsg, { t: "tunnel" }>;

const COLOR: Record<Faction, string> = { fire: "#ff7a1a", water: "#2f9bff" };

function distance(m: number): string {
  return m < 1000 ? `${Math.round(m / 10) * 10} m` : `${(m / 1000).toLocaleString(lang, { maximumFractionDigits: 1 })} km`;
}

/** `view(true)` when the tunnel wants the map, `view(false)` when it is over. */
export function setupTunnel(
  map: maplibregl.Map,
  me: () => [number, number] | null,
  claim: (node: number) => void,
  view: (on: boolean) => void,
  toast: (text: string) => void,
) {
  let open: (TunnelMsg & { until: number; total: number }) | null = null;
  let timer = 0;
  const box = $("tunnel");
  map.addSource("tunnel", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({
    id: "tunnel-line",
    type: "line",
    source: "tunnel",
    filter: ["==", ["geometry-type"], "LineString"],
    paint: { "line-color": "#ffd43b", "line-width": 3, "line-dasharray": [1, 1.5], "line-opacity": 0.85 },
  });
  map.addLayer({
    id: "tunnel-end",
    type: "circle",
    source: "tunnel",
    filter: ["==", ["geometry-type"], "Point"],
    paint: {
      "circle-radius": 11,
      "circle-color": ["get", "color"],
      "circle-opacity": 0.35,
      "circle-stroke-color": "#ffd43b",
      "circle-stroke-width": 3,
    },
  });

  function draw() {
    const p = me();
    const src = map.getSource("tunnel") as maplibregl.GeoJSONSource;
    if (!open || !p) return src.setData({ type: "FeatureCollection", features: [] });
    const end: [number, number] = [open.lon, open.lat];
    const holder: Faction = open.kind === "peak" ? "fire" : "water";
    src.setData({
      type: "FeatureCollection",
      features: [
        { type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: [p, end] } },
        { type: "Feature", properties: { color: COLOR[holder] }, geometry: { type: "Point", coordinates: end } },
      ],
    });
  }

  function tick() {
    if (!open) return;
    const left = (open.until - Date.now()) / 1000;
    if (left <= 0) return close(t("tunnel.gone"));
    $("tunnel-left").textContent = t("tunnel.left", { s: Math.ceil(left) });
    ($("tunnel-bar") as HTMLElement).style.width = `${(100 * left) / open.total}%`;
  }

  /** `linger`: the map stays at the far end this long before it returns to the player. */
  function close(note?: string, linger = 0) {
    clearInterval(timer);
    const was = !!open;
    open = null;
    box.hidden = true;
    draw();
    if (was) setTimeout(() => view(false), linger);
    if (was && note) toast(note);
  }

  // Off to the far end: the claim shows there (it builds up on enemy ground).
  $("tunnel-go").onclick = () => {
    if (!open) return;
    map.flyTo({ center: [open.lon, open.lat], zoom: Math.max(map.getZoom(), 16.5), duration: 1500, essential: true });
    claim(open.node);
  };
  $("tunnel-close").onclick = () => close();

  return {
    onTunnel(m: TunnelMsg) {
      const p = me();
      open = { ...m, until: Date.now() + m.secs * 1000, total: m.secs };
      const dir = p ? ` ${arrow(p, [m.lon, m.lat])}` : "";
      $("tunnel-text").textContent = t(m.kind === "peak" ? "tunnel.peak" : "tunnel.sink", { d: `${distance(m.m)}${dir}` });
      box.hidden = false;
      tick();
      timer = window.setInterval(tick, 250);
      draw();
      view(true);
      if (p) map.fitBounds(new maplibregl.LngLatBounds(p, p).extend([m.lon, m.lat]), { padding: { top: 140, bottom: 300, left: 50, right: 70 }, maxZoom: 17, duration: 900 });
      navigator.vibrate?.([60, 40, 60, 40, 120]);
    },
    /** A claim answer for the tunnel's node: true if it was the tunnel's. */
    onClaim(node: number, ok: boolean): boolean {
      if (!open || open.node !== node) return false;
      if (ok) close(undefined, 4000);
      return true;
    },
    /** The player moved: the line follows. */
    moved: draw,
  };
}
