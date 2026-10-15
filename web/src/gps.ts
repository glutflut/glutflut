// Position input: the GPS watch (with the iOS Safari quirks), or tap-to-move
// on test servers. Starts with the first welcome, since only the server says
// whether tap-to-move is allowed.
import * as maplibregl from "maplibre-gl";
import { t, type Key } from "./i18n";
import { $, params, toast } from "./page";

export function setupGps(
  map: maplibregl.Map,
  setMe: (p: [number, number], accuracy?: number, speed?: number) => void,
  /** A tap on the map (sim: right click / long press) opens the junction's card. */
  tapCard: (p: maplibregl.Point) => void,
) {
  let inputStarted = false;
  function start(sim: boolean) {
    if (inputStarted) return;
    inputStarted = true;
    if (sim) {
      $("gps").textContent = t("gps.sim");
      map.on("click", (e) => setMe([e.lngLat.lng, e.lngLat.lat]));
      // A tap moves here: the card on right click / long press.
      map.on("contextmenu", (e) => tapCard(e.point));
      const at = params.get("at")?.split(",").map(Number);
      if (at?.length === 2) setMe([at[1], at[0]]);
    } else {
      map.on("click", (e) => tapCard(e.point));
      startGps();
    }
  }

  // iOS Safari can leave a location request unanswered (no prompt, no error):
  // the timeout turns that into a hint, and a tap asks again from a gesture.
  let gpsWatch: number | null = null;
  let gpsFixed = false;
  const gpsEl = $("gps");
  const gpsHint = (k: Key) => {
    gpsEl.textContent = t(k);
    gpsEl.classList.add("tap");
  };
  function startGps() {
    if (!("geolocation" in navigator)) return void (gpsEl.textContent = t("gps.none"));
    if (gpsWatch !== null) navigator.geolocation.clearWatch(gpsWatch);
    if (!gpsFixed) gpsEl.textContent = t("gps.searching");
    let last = 0;
    gpsWatch = navigator.geolocation.watchPosition(
      (p) => {
        const now = Date.now();
        if (now - last < 1000) return;
        last = now;
        gpsFixed = true;
        gpsEl.classList.remove("tap");
        // Speed only from a good fix; a poor one's speed is noise.
        const v = p.coords.speed;
        const speed = v !== null && Number.isFinite(v) && p.coords.accuracy <= 40 ? v : undefined;
        setMe([p.coords.longitude, p.coords.latitude], p.coords.accuracy, speed);
      },
      // Desktop browsers often have no location at all (no GPS; Chromium without
      // Google keys gets a 403 from the network location service).
      (err) => {
        if (err.code === err.PERMISSION_DENIED) gpsHint("gps.denied");
        else if (err.code !== err.TIMEOUT) gpsHint("gps.unavailable");
        else if (!gpsFixed) gpsHint("gps.timeout");
      },
      { enableHighAccuracy: true, maximumAge: 2000, timeout: 15_000 },
    );
  }
  gpsEl.onclick = () => {
    if (!gpsEl.classList.contains("tap")) return;
    const ios = /iP(hone|ad|od)/.test(navigator.userAgent) || (/Mac/.test(navigator.userAgent) && navigator.maxTouchPoints > 1);
    toast(t(ios ? "gps.help.ios" : "gps.help"), 12_000);
    startGps();
  };
  // Allowed later in the settings: start again without a reload.
  navigator.permissions
    ?.query({ name: "geolocation" })
    .then((s) => (s.onchange = () => s.state === "granted" && inputStarted && !gpsFixed && startGps()))
    .catch(() => {});

  return { start };
}
