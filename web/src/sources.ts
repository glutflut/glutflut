// Claimed sources on the map (web.rs `Sources`): a ring in the side's colour
// whose arc shrinks with the time left, which with linear decay is also its
// strength; yours get a white rim. While a claim builds up on enemy ground
// (`ready` in the future): a turning dashed ring, an arc filling up and a
// growing dot (yours: seconds left). Hinterland: a spark pulses with a
// diamond, an anchor has a square, a source cut off from supply a dashed ring.
import * as maplibregl from "maplibre-gl";
import type { Faction, ServerMsg, SourceKind } from "./net";
import { COLORS } from "./geo";
import { mottoText } from "./mottos";

export type Src = {
  node: number;
  faction: Faction;
  until: number;
  full: number;
  mine: boolean;
  ll: [number, number];
  ready: number;
  ramp: number;
  kind: SourceKind;
  nokeep: boolean;
  /** A permanent source's tag line (key, `mottos.rs`). */
  motto: string | null;
  /** Inside a ring of the other side's players: ages faster (`siege.rs`). */
  sieged: boolean;
  /** A permanent source linked with another of its owner's (`keep.rs`). */
  linked: boolean;
};
export type SourceItems = Extract<ServerMsg, { t: "sources" }>["items"];

/** A row of `sources`/`sourcesdiff`; `own`: the node of this device's permanent source. */
function toSrc(
  [node, faction, left, full, mine, lon, lat, build = 0, ramp = 0, kind = "", nokeep = false, motto = null, sieged = false, linked = false]: SourceItems[number],
  own: ReadonlySet<number>,
  now: number,
): Src {
  return {
    node,
    faction,
    until: now + left * 1000,
    full: Math.max(full, left, 1),
    // Permanent sources carry no owner; this device knows its own.
    mine: mine || (kind === "keep" && own.has(node)),
    ll: [lon, lat],
    ready: now + build * 1000,
    ramp: Math.max(ramp, build),
    kind,
    nokeep,
    motto: kind === "keep" ? motto : null,
    sieged,
    linked,
  };
}

export const building = (s: Src, now: number) => s.ready > now && s.ramp > 0;
const srcKey = (s: Src, now: number) => `${s.faction}${s.mine}${building(s, now)}${s.kind}${s.sieged}${s.linked}`;
const ARC_R = 14;
const ARC_C = 2 * Math.PI * ARC_R;

export function setupSources(map: maplibregl.Map) {
  let sources: Src[] = [];
  // Tag lines of permanent sources: labels for everyone, close up only.
  map.addSource("mottos", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({
    id: "mottos",
    type: "symbol",
    source: "mottos",
    minzoom: 16,
    layout: {
      "text-field": ["get", "text"],
      "text-font": ["Noto Sans Medium"],
      "text-size": 12,
      "text-offset": [0, 1.6],
      "text-anchor": "top",
      "text-max-width": 12,
      "text-optional": true,
    },
    paint: { "text-color": ["get", "color"], "text-halo-color": "#16181d", "text-halo-width": 1.6 },
  });
  let mottoKey = "";
  function drawMottos() {
    const withMotto = sources.filter((s) => s.motto && mottoText(s.motto));
    const key = withMotto.map((s) => `${s.node}:${s.motto}`).join(",");
    if (key === mottoKey) return;
    mottoKey = key;
    (map.getSource("mottos") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: withMotto.map((s) => ({
        type: "Feature",
        properties: { text: mottoText(s.motto!), color: COLORS[s.faction] },
        geometry: { type: "Point", coordinates: s.ll },
      })),
    });
  }
  const sourceMarkers = new Map<
    number,
    { marker: maplibregl.Marker; arc: SVGCircleElement; dot: SVGCircleElement | null; secs: SVGTextElement | null; key: string }
  >();
  function sourceMarker(s: Src, now: number) {
    const color = COLORS[s.faction];
    const el = document.createElement("div");
    el.className = "source";
    if (building(s, now)) {
      el.innerHTML =
        `<svg viewBox="0 0 40 40" width="40" height="40">` +
        `<circle class="spin" cx="20" cy="20" r="18" fill="none" stroke="${s.mine ? "#fff" : color}" stroke-width="2" stroke-dasharray="4 4.5"/>` +
        `<circle cx="20" cy="20" r="${ARC_R}" fill="${color}" fill-opacity="0.12" stroke="${color}" stroke-opacity="0.22" stroke-width="5"/>` +
        `<circle class="arc" cx="20" cy="20" r="${ARC_R}" fill="none" stroke="${color}" stroke-width="5" ` +
        `stroke-dasharray="${ARC_C} ${ARC_C}" stroke-dashoffset="${ARC_C}" transform="rotate(-90 20 20)"/>` +
        (s.mine
          ? `<text class="secs" x="20" y="24" text-anchor="middle"></text>`
          : `<circle class="dot" cx="20" cy="20" r="2" fill="${color}" stroke="#fff" stroke-width="1.5"/>`) +
        `</svg>`;
      const marker = new maplibregl.Marker({ element: el }).setLngLat(s.ll).addTo(map);
      return {
        marker,
        arc: el.querySelector<SVGCircleElement>(".arc")!,
        dot: el.querySelector<SVGCircleElement>(".dot"),
        secs: el.querySelector<SVGTextElement>(".secs"),
        key: srcKey(s, now),
      };
    }
    if (s.kind === "cut") el.classList.add("cut");
    if (s.sieged) el.classList.add("sieged");
    const centre =
      s.kind === "spark"
        ? `<path d="M20 13 L27 20 L20 27 L13 20 Z" fill="${color}" stroke="#fff" stroke-width="2"/>`
        : s.kind === "anchor"
          ? `<rect x="14.5" y="14.5" width="11" height="11" rx="2" fill="${color}" stroke="#fff" stroke-width="2"/>`
          : s.kind === "keep"
          ? `<path d="M20 11 L27.8 15.5 L27.8 24.5 L20 29 L12.2 24.5 L12.2 15.5 Z" fill="${color}" stroke="#fff" stroke-width="2"/>`
          : `<circle cx="20" cy="20" r="5" fill="${color}" stroke="#fff" stroke-width="2"/>`;
    el.innerHTML =
      `<svg viewBox="0 0 40 40" width="40" height="40">` +
      (s.mine ? `<circle cx="20" cy="20" r="18.5" fill="none" stroke="#fff" stroke-width="3"/>` : "") +
      // Linked permanent source: a dotted ring (not with which one).
      (s.linked ? `<circle cx="20" cy="20" r="14" fill="none" stroke="#fff" stroke-width="1.5" stroke-dasharray="2 2.5"/>` : "") +
      (s.kind === "spark" ? `<circle class="pulse" cx="20" cy="20" r="17" fill="none" stroke="${color}" stroke-width="2.5"/>` : "") +
      `<circle cx="20" cy="20" r="${ARC_R}" fill="${color}" fill-opacity="0.18" stroke="${color}" stroke-opacity="0.3" stroke-width="5"` +
      (s.kind === "cut" ? ` stroke-dasharray="3 3"` : "") +
      `/>` +
      `<circle class="arc" cx="20" cy="20" r="${ARC_R}" fill="none" stroke="${color}" stroke-width="5" ` +
      `stroke-dasharray="${ARC_C} ${ARC_C}" transform="rotate(-90 20 20)"/>` +
      centre +
      `</svg>`;
    const marker = new maplibregl.Marker({ element: el }).setLngLat(s.ll).addTo(map);
    return { marker, arc: el.querySelector<SVGCircleElement>(".arc")!, dot: null, secs: null, key: srcKey(s, now) };
  }
  function draw() {
    const now = Date.now();
    // Permanent sources stay at their floor once faded.
    sources = sources.filter((s) => s.kind === "keep" || s.until > now);
    // Markers only near the view (a whole city can have hundreds).
    const b = map.getBounds();
    const pad = [b.getEast() - b.getWest(), b.getNorth() - b.getSouth()];
    const near = (s: Src) =>
      s.ll[0] > b.getWest() - pad[0] && s.ll[0] < b.getEast() + pad[0] && s.ll[1] > b.getSouth() - pad[1] && s.ll[1] < b.getNorth() + pad[1];
    const shown = map.getZoom() >= 12 ? sources.filter(near) : [];
    const live = new Set(shown.map((s) => s.node));
    for (const [node, m] of sourceMarkers) {
      if (!live.has(node)) {
        m.marker.remove();
        sourceMarkers.delete(node);
      }
    }
    for (const s of shown) {
      let m = sourceMarkers.get(s.node);
      // Taken over (or built up): new look.
      if (m && m.key !== srcKey(s, now)) {
        m.marker.remove();
        m = undefined;
      }
      if (!m) sourceMarkers.set(s.node, (m = sourceMarker(s, now)));
      if (building(s, now)) {
        const p = 1 - (s.ready - now) / (s.ramp * 1000);
        m.arc.setAttribute("stroke-dashoffset", String(ARC_C * (1 - p)));
        m.dot?.setAttribute("r", String(2 + 4 * p));
        if (m.secs) m.secs.textContent = String(Math.ceil((s.ready - now) / 1000));
        continue;
      }
      const life = Math.min(1, Math.max(0, (s.until - now) / (s.full * 1000)));
      m.arc.setAttribute("stroke-dashoffset", String(ARC_C * (1 - life)));
    }
  }
  // Often while something builds up (arc, seconds), else every 2 s.
  let lastSources = 0;
  setInterval(() => {
    const now = Date.now();
    if (now - lastSources >= 2000 || sources.some((s) => building(s, now))) {
      lastSources = now;
      draw();
    }
  }, 500);

  return {
    all: () => sources,
    draw,
    /** The server's list; `own`: the nodes of this device's permanent sources. */
    set(items: SourceItems, own: ReadonlySet<number>) {
      const now = Date.now();
      sources = items.map((i) => toSrc(i, own, now));
      draw();
      drawMottos();
    },
    /** Changes (`sourcesdiff`): `set` replaces by node, `gone` removes. */
    apply(set: SourceItems, gone: number[], own: ReadonlySet<number>) {
      const now = Date.now();
      const drop = new Set([...gone, ...set.map((i) => i[0])]);
      sources = sources.filter((s) => !drop.has(s.node)).concat(set.map((i) => toSrc(i, own, now)));
      draw();
      drawMottos();
    },
    /** Junctions of tiles let go: no more news about them. */
    forget(nodes: Iterable<number>) {
      const gone = new Set(nodes);
      if (gone.size) {
        sources = sources.filter((s) => !gone.has(s.node));
        drawMottos();
      }
    },
  };
}
