// Hooks for scripted recordings of the app (explainer video, pitch intro),
// sim mode or `?video` only: `window.__video`.
import * as maplibregl from "maplibre-gl";
import qrcode from "qrcode-generator";
import type { Faction } from "./net";
import { distM } from "./geo";
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
