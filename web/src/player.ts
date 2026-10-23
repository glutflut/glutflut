// The player on the map: position marker and claim range, follow mode (the
// map moves with the player until they pan it themselves), the "me" and
// "city" buttons, and each position sent to the server (web.rs `Pos`).
import * as maplibregl from "maplibre-gl";
import { t } from "./i18n";
import type { Net } from "./net";
import { checkRegion } from "./outside";
import { presenceToSend } from "./presence";
import type { Meta, TileLayer } from "./tiles";
import { circle } from "./geo";
import { $, params, toast } from "./page";

export function setupPlayer(d: {
  map: maplibregl.Map;
  meta: Meta;
  net: Net;
  tiles: TileLayer;
  claimRadius: () => number;
  /** After each position: the tunnel, the claim button, the hotspot hint. */
  moved: () => void;
}) {
  const { map, net } = d;
  let me: [number, number] | null = null;
  const marker = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "me-marker" }) });

  let follow = !params.has("sim");
  const setFollow = (on: boolean) => {
    follow = on;
    $("nav-me").classList.toggle("follow", on);
  };
  setFollow(follow);
  map.on("dragstart", () => setFollow(false));
  /** To the player; from far away (the globe) a flight, not a jump. */
  const goHome = () => {
    if (!me) return false;
    setFollow(true);
    const zoom = Math.max(map.getZoom(), 16.5);
    if (zoom - map.getZoom() > 4) map.flyTo({ center: me, zoom, duration: 2500, essential: true });
    else map.easeTo({ center: me, zoom });
    return true;
  };
  $("nav-me").onclick = () => {
    if (!goHome()) toast(t("gps.searching"));
  };
  $("nav-city").onclick = () => {
    setFollow(false);
    map.fitBounds(d.meta.region.bounds, { padding: 40 });
  };

  const drawRange = () => me && (map.getSource("range") as maplibregl.GeoJSONSource).setData(circle(me, d.claimRadius()));

  /** `speed`: the device's own speed in m/s (GPS Doppler), for the driving check. */
  function setMe(p: [number, number], accuracy?: number, speed?: number) {
    if (!me) map.jumpTo({ center: p, zoom: 16.5 });
    else if (follow) map.easeTo({ center: p, duration: 600 });
    me = p;
    marker.setLngLat(p).addTo(map);
    drawRange();
    $("gps").textContent = accuracy ? t("gps.accuracy", { m: Math.round(accuracy) }) : t("gps.set");
    // A presence code from the game link, once there is a position.
    const code = presenceToSend();
    queueMicrotask(() => code && net.send({ t: "presence", code }));
    net.send({
      t: "pos",
      lat: p[1],
      lon: p[0],
      ...(speed !== undefined ? { spd: speed } : {}),
      // Accuracy for the plausibility check (mock apps report a constant one).
      ...(accuracy !== undefined ? { acc: Math.round(accuracy * 10) / 10 } : {}),
    });
    d.tiles.setPlayer(p);
    // Test servers: the tapped position stays in the address, so a reload
    // (language, graphics) continues here.
    if (params.has("sim")) {
      const u = new URL(location.href);
      u.searchParams.set("at", `${p[1].toFixed(6)},${p[0].toFixed(6)}`);
      history.replaceState(history.state, "", u);
    }
    d.moved();
    checkRegion(p);
  }

  return {
    me: () => me,
    marker,
    setMe,
    drawRange,
    follow: () => follow,
    setFollow,
    /** Back to the player (after a tunnel, a flight elsewhere, the globe). */
    center() {
      goHome();
    },
    /** Like `center`, saying whether there was a position to go to. */
    home: goHome,
  };
}
