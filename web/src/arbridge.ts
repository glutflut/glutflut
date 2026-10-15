// Camera view (AR, ar.ts): an experiment (compasses are often off), so its
// button shows only after switching it on in the settings; loaded on demand.
// Feeds the view the streets, sources and ground height around the player.
import { t, type Key } from "./i18n";
import type { FlowLayer } from "./flow";
import type { TileLayer } from "./tiles";
import { building, type Src } from "./sources";
import { distM } from "./geo";
import { $, toast } from "./page";
import { KEYS, load, save } from "./store";

export function setupArBridge(d: {
  me: () => [number, number] | null;
  tiles: TileLayer;
  edgeState: Map<number, { o: number; s: number }>;
  sources: () => Src[];
  flow: () => FlowLayer | null;
}) {
  const { tiles } = d;
  const arToggle = $("ar-on") as HTMLInputElement;
  arToggle.checked = load(KEYS.ar) === "1";
  $("nav-ar").hidden = !arToggle.checked;
  arToggle.onchange = () => {
    $("nav-ar").hidden = !arToggle.checked;
    save(KEYS.ar, arToggle.checked ? "1" : "0");
  };
  let closeAr: (() => void) | null = null;
  $("nav-ar").onclick = async () => {
    if (closeAr) {
      closeAr();
      closeAr = null;
      $("nav-ar").classList.remove("on");
      $("map").style.visibility = "";
      d.flow()?.pause(false);
      return;
    }
    if (!d.me()) return toast(t("gps.searching"));
    $("nav-ar").classList.add("on");
    const { openAr } = await import("./ar");
    d.flow()?.pause(true);
    $("map").style.visibility = "hidden";
    closeAr = await openAr($("ar"), {
      me: d.me,
      edges: (c, r) => {
        const out = [];
        for (const [id, line] of tiles.edgeLine) {
          let near = false;
          for (let i = 0; i + 1 < line.length && !near; i += 2) near = distM(c, [line[i], line[i + 1]]) <= r;
          const e = tiles.edgeById.get(id);
          if (!near || !e) continue;
          const st = d.edgeState.get(id) ?? { o: 0, s: 0 };
          out.push({ line, o: st.o, s: st.s, h0: tiles.nodeElev.get(e.source) ?? 0, h1: tiles.nodeElev.get(e.target) ?? 0 });
        }
        return out;
      },
      sources: () => {
        const now = Date.now();
        return d.sources().map((s) => ({ ll: s.ll, faction: s.faction, building: building(s, now) }));
      },
      ground: (at) => {
        let best = Infinity;
        let h = 0;
        for (const [id, p] of tiles.nodePos) {
          const dist = distM(at, p);
          if (dist < best) [best, h] = [dist, tiles.nodeElev.get(id) ?? 0];
        }
        return h;
      },
      text: (k, v) => t(`ar.${k}` as Key, v),
    });
  };
}
