// Hooks for scripted recordings of the app (explainer video, pitch intro),
// sim mode or `?video` only: `window.__video`.
import * as maplibregl from "maplibre-gl";
import qrcode from "qrcode-generator";
import type { Faction } from "./net";
import type { BurstKind } from "./burst";
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
  burst: (node: number, side: Faction, kind: BurstKind) => boolean;
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
      /** The claim animation (only the picture, no claim) at a random loaded
       *  junction within `r` m of `at`; where it fired, or null. */
      burstNear: ([at, r, side, kind]: [[number, number], number, Faction, BurstKind]) => {
        const near = [...d.nodePos].filter(([, p]) => distM(at, p) <= r);
        for (let tries = 0; tries < 8 && near.length; tries++) {
          const [id, p] = near.splice(Math.floor(Math.random() * near.length), 1)[0];
          if (d.burst(id, side, kind)) return p;
        }
        return null;
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
