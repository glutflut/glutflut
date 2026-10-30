// Live action while watching (watch.ts) and on screens (`?kiosk`): fresh
// sources flash in their side's colour, the top bar counts the claims of the
// last minute, and the kiosk camera tours where things happen.
import * as maplibregl from "maplibre-gl";
import qrcode from "qrcode-generator";
import { t } from "./i18n";
import { COLORS } from "./geo";
import { $ } from "./page";
import type { Faction, SourceRow } from "./protocol";

/** A source younger than this (seconds of its life used) counts as a claim just now. */
const FRESH_S = 20;
const WINDOW_MS = 60_000;
/** At most this many flashes on the map at once (busy squares stay calm). */
const FLASH_MAX = 12;

/** Fresh sources among `sources` rows (claims just now), each once. */
export function freshSources() {
  const seen = new Map<number, number>();
  return (items: SourceRow[]) => {
    const now = Date.now();
    const out: { node: number; ll: [number, number]; side: Faction }[] = [];
    for (const [node, side, left, life, , lon, lat] of items) {
      if (life - left > FRESH_S) continue;
      // The same source again (a view load, a refresh): once is enough.
      const before = seen.get(node);
      if (before && now - before < 30_000) continue;
      seen.set(node, now);
      out.push({ node, ll: [lon, lat], side });
    }
    if (seen.size > 2000) for (const [k, v] of seen) if (now - v > 60_000) seen.delete(k);
    return out;
  };
}

/** `burst`: draws a fresh claim instead of the flash (burst.ts); false if it couldn't. */
export function setupLive(map: maplibregl.Map, label: () => string, burst?: (node: number, side: Faction) => boolean) {
  const recent: { at: number; ll: [number, number]; side: Faction }[] = [];
  const fresh = freshSources();
  let flashes = 0;

  function flash(ll: [number, number], side: Faction) {
    if (flashes >= FLASH_MAX) return;
    flashes++;
    // MapLibre places the marker by its transform: the animation goes on a child.
    const el = document.createElement("div");
    el.innerHTML = `<div class="live-flash"></div>`;
    el.style.setProperty("--c", COLORS[side]);
    const m = new maplibregl.Marker({ element: el }).setLngLat(ll).addTo(map);
    setTimeout(() => {
      m.remove();
      flashes--;
    }, 1600);
  }

  function count() {
    const now = Date.now();
    while (recent.length && now - recent[0].at > WINDOW_MS) recent.shift();
    const n = recent.length;
    $("me-sub").textContent = n ? `${label()} · ${t(n === 1 ? "live.one" : "live.n", { n })}` : label();
  }
  setInterval(count, 5_000);

  /** Rows of `sources` / `sourcesdiff`: the fresh ones are claims just now. */
  function rows(items: SourceRow[]) {
    const now = Date.now();
    for (const { node, ll, side } of fresh(items)) {
      recent.push({ at: now, ll, side });
      if (!burst?.(node, side)) flash(ll, side);
    }
    count();
  }

  return { rows, recent: () => recent.filter((r) => Date.now() - r.at < WINDOW_MS) };
}

/** Kiosk: no buttons, a QR code, and a camera that moves on every ~20 s. */
export function startKiosk(
  map: maplibregl.Map,
  centre: [number, number],
  live: ReturnType<typeof setupLive>,
  hotspots: () => [number, number][],
  lapse?: { open(): Promise<void>; close(): void },
  rank?: { showRank(): Promise<void>; hideRank(): void },
) {
  document.body.classList.add("kiosk");
  const q = qrcode(0, "M");
  q.addData(`${location.origin}/tv`);
  q.make();
  $("kiosk-qr-code").innerHTML = q.createSvgTag({ scalable: true, margin: 0 });
  $("kiosk-qr").hidden = false;
  // Keep the screen on where the browser allows it.
  const wake = () => (navigator as Navigator & { wakeLock?: { request(k: "screen"): Promise<unknown> } }).wakeLock?.request("screen").catch(() => {});
  void wake();
  document.addEventListener("visibilitychange", () => !document.hidden && void wake());

  let step = 0;
  let bearing = 0;
  const next = () => {
    step++;
    // Every fifth stop: the last day as a time-lapse (timelapse.ts), then live again.
    if (lapse && step % 5 === 0) {
      void lapse.open();
      setTimeout(() => lapse.close(), 16_000);
      return;
    }
    // Every fifth stop too, between the time-lapses: the ranking.
    if (rank && step % 5 === 3) {
      void rank.showRank();
      setTimeout(() => rank.hideRank(), 14_000);
      return;
    }
    // Where something happened lately, else a hotspot, else the centre.
    const fresh = live.recent();
    const hot = hotspots();
    let at: [number, number];
    let zoom = 16;
    if (fresh.length && step % 3 !== 0) at = fresh[fresh.length - 1].ll;
    else if (hot.length && step % 3 === 1) at = hot[step % hot.length];
    else [at, zoom] = [centre, 14.5];
    bearing = (bearing + 40 + Math.random() * 40) % 360;
    map.flyTo({ center: at, zoom, pitch: 50, bearing, duration: 9000, essential: true });
  };
  next();
  setInterval(next, 20_000);
}
