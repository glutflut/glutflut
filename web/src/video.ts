// Hooks for scripted recordings of the app (explainer video, pitch intro),
// sim mode or `?video` only: `window.__video`.
import * as maplibregl from "maplibre-gl";
import qrcode from "qrcode-generator";
import type { Faction } from "./net";
import type { BurstKind } from "./burst";
import { COLORS, distM } from "./geo";
import { parse, runAt, type Lapse } from "./timelapse";
import { params } from "./page";

export function setupVideo(d: {
  map: maplibregl.Map;
  me: () => [number, number] | null;
  faction: () => Faction | null;
  setMe: (ll: [number, number]) => void;
  hotspots: () => [number, number][];
  lastClaim: () => [number, number] | null;
  state: Map<number, [number, number]>;
  nodePos: Map<number, [number, number]>;
  burst: (node: number, side: Faction, kind: BurstKind) => boolean;
  /** A source ring as after a claim (only on this screen). */
  stage: (node: number, side: Faction) => void;
}) {
  if (!params.has("sim") && !params.has("video")) return;
  const { map } = d;
  const want = params.get("side");
  Object.assign(window, {
    __video: {
      ready: () => !!d.me() && !!d.faction() && (!want || d.faction() === want),
      jump: (o: maplibregl.JumpToOptions) => map.jumpTo(o),
      moveTo: (ll: [number, number]) => d.setMe(ll),
      hotspot: () => d.hotspots()[0] ?? null,
      lastClaim: () => d.lastClaim(),
      /** The loaded node within `r` m of `at` where the other side is strongest. */
      enemyNear: ([at, r]: [[number, number], number]) => {
        let best: [number, number] | null = null;
        let top = 0;
        for (const [id, [f, w]] of d.state) {
          const p = d.nodePos.get(id);
          const enemy = d.faction() === "water" ? f : w;
          if (p && enemy > top && distM(at, p) <= r) [best, top] = [p, enemy];
        }
        return best;
      },
      /** A claim's picture (animation and source ring, no claim) at a random
       *  loaded junction within `r` m of `at`; where it fired, or null. */
      burstNear: ([at, r, side, kind]: [[number, number], number, Faction, BurstKind]) => {
        const near = [...d.nodePos].filter(([, p]) => distM(at, p) <= r);
        for (let tries = 0; tries < 8 && near.length; tries++) {
          const [id, p] = near.splice(Math.floor(Math.random() * near.length), 1)[0];
          if (d.burst(id, side, kind)) {
            d.stage(id, side);
            return p;
          }
        }
        return null;
      },
      /** The game state now as hexagons (the globe files): every region
       *  coarse while zoomed out, `slug` fine closer in, gone by zoom 13.6. */
      hexes: (slug: string) => hexes(map, slug),
      fly: (o: maplibregl.FlyToOptions) => map.flyTo({ ...o, essential: true }),
      qr: (url: string) => {
        const q = qrcode(0, "M");
        q.addData(url);
        q.make();
        return q.createSvgTag({ scalable: true, margin: 0 });
      },
    },
  });
}

/** Features of one globe file at its last started bin, by the leading side. */
function now(l: Lapse): GeoJSON.Feature[] {
  const last = Math.min(l.bins - 1, Math.floor((Date.now() / 60_000 - l.startMin) / l.binMin));
  const out: GeoJSON.Feature[] = [];
  l.cells.forEach((c, i) => {
    const r = runAt(c, last);
    if (r < 0) return;
    const [f, w, fr] = [c.fire[r], c.water[r], c.front[r]];
    const lead = Math.max(f, w, fr);
    if (!lead) return;
    const color = lead === f ? COLORS.fire : lead === w ? COLORS.water : COLORS.front;
    out.push({ ...l.geo.features[i], properties: { color, a: Math.min(1, (1.6 * lead) / 255) } });
  });
  return out;
}

async function hexes(map: maplibregl.Map, slug: string) {
  const m = await fetch("/api/marble/manifest.json").then((r) => r.json());
  const regions = m.regions as { slug: string; files: Record<string, string> }[];
  const load = (file?: string) =>
    file ? fetch(`/api/marble/${file}`).then((r) => r.arrayBuffer()).then((b) => now(parse(b))).catch(() => []) : Promise.resolve([]);
  const far = (await Promise.all(regions.map((r) => load(r.files["7"])))).flat();
  const near = await load(regions.find((r) => r.slug === slug)?.files["9"]);
  // [layer, features, zoom stops of the opacity factor]
  const layers: [string, GeoJSON.Feature[], number[]][] = [
    ["vhex-far", far, [3, 0.9, 9.2, 0.9, 10.4, 0]],
    ["vhex-near", near, [9.2, 0, 10.4, 0.8, 12.6, 0.8, 13.6, 0]],
  ];
  const below = map.getLayer("edges-glow") ? "edges-glow" : undefined;
  for (const [id, features, stops] of layers) {
    map.addSource(id, { type: "geojson", data: { type: "FeatureCollection", features } });
    const z: (number | maplibregl.ExpressionSpecification)[] = [];
    for (let i = 0; i < stops.length; i += 2) z.push(stops[i], ["*", stops[i + 1], ["get", "a"]]);
    map.addLayer(
      {
        id,
        type: "fill",
        source: id,
        paint: {
          "fill-color": ["get", "color"],
          "fill-opacity": ["interpolate", ["linear"], ["zoom"], ...z] as maplibregl.ExpressionSpecification,
          "fill-outline-color": "rgba(11,14,19,0.6)",
        },
      },
      below,
    );
  }
  // The squares of the overview would cover them.
  if (map.getLayer("overview")) map.setPaintProperty("overview", "fill-opacity", 0);
  return { far: far.length, near: near.length };
}
