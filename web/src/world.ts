// Element state per junction and street as the server sends it (web.rs
// `State`), applied as feature-state; the flow layer gets the streets too.
import * as maplibregl from "maplibre-gl";
import type { FlowLayer } from "./flow";
import type { TileLayer } from "./tiles";
import { owner } from "./geo";

export function setupWorld(map: maplibregl.Map, flow: () => FlowLayer | null, areaShare: number | null = null) {
  const state = new Map<number, [number, number]>();
  /** Owner code and strength per edge (for the camera view). */
  const edgeState = new Map<number, { o: number; s: number }>();

  /** Blocks: filled for the side holding `areaShare` of the border and none
   * of the other side's; `s` the mean strength of the holder's border. */
  function blocks(ids: Iterable<number>, tiles: TileLayer) {
    if (areaShare === null) return;
    for (const id of ids) {
      const border = tiles.blockNodes.get(id);
      if (!border) continue;
      let fire = 0;
      let water = 0;
      let fs = 0;
      let ws = 0;
      for (const n of border) {
        const s = state.get(n);
        const o = s ? owner(s[0], s[1]) : 0;
        if (s) {
          fs += s[0];
          ws += s[1];
        }
        if (o === 1) fire++;
        else if (o === 2) water++;
      }
      const need = Math.max(1, Math.ceil(areaShare * border.length - 1e-6));
      const o = water === 0 && fire >= need ? 1 : fire === 0 && water >= need ? 2 : 0;
      const s = o === 1 ? fs / border.length : o === 2 ? ws / border.length : 0;
      map.setFeatureState({ source: "blocks", id }, { o, s: Math.round(Math.min(1, s) * 20) / 20 });
    }
  }

  function apply(nodes: [number, number, number][], tiles: TileLayer) {
    const touched = new Set<number>();
    const touchedBlocks = new Set<number>();
    for (const [id, f, w] of nodes) {
      if (!tiles.nodePos.has(id)) continue; // tile dropped meanwhile
      state.set(id, [f, w]);
      map.setFeatureState({ source: "nodes", id }, { o: owner(f, w), s: Math.max(f, w) });
      for (const e of tiles.edgesOf.get(id) ?? []) touched.add(e);
      for (const b of tiles.blocksOf.get(id) ?? []) touchedBlocks.add(b);
    }
    blocks(touchedBlocks, tiles);
    for (const eid of touched) {
      const e = tiles.edgeById.get(eid);
      if (!e) continue;
      const [f1, w1] = state.get(e.source) ?? [0, 0];
      const [f2, w2] = state.get(e.target) ?? [0, 0];
      const f = (f1 + f2) / 2;
      const w = (w1 + w2) / 2;
      const o = owner(f, w);
      map.setFeatureState({ source: "edges", id: eid }, { o, s: Math.max(f, w) });
      edgeState.set(eid, { o, s: Math.max(f, w) });
      flow()?.set(eid, o, Math.max(f, w));
    }
  }

  /** Blocks held with players' sources (property `p` 1), yours among them
   * (`p` 2); from the server every minute (web.rs `Areas`). A property, not a
   * feature-state: the hatching is a fill pattern. */
  let players = new Map<number, number>();
  function areas(player: number[], mine: number[], tiles: TileLayer) {
    const src = map.getSource("blocks") as maplibregl.GeoJSONSource | undefined;
    if (areaShare === null || !src) return;
    const now = new Map<number, number>(player.map((id) => [id, 1]));
    for (const id of mine) now.set(id, 2);
    const changed = new Set([...now.keys(), ...players.keys()].filter((id) => now.get(id) !== players.get(id)));
    players = now;
    const update = [...changed]
      .filter((id) => tiles.blockNodes.has(id))
      .map((id) => ({ id, addOrUpdateProperties: [{ key: "p", value: now.get(id) ?? 0 }] }));
    if (update.length) src.updateData({ update });
  }
  const blockProps = (id: number) => ({ p: players.get(id) ?? 0 });

  /** Tiles gone out of view: their junctions are forgotten. */
  function drop(gone: Iterable<number>) {
    for (const id of gone) {
      state.delete(id);
      map.removeFeatureState({ source: "nodes", id });
    }
  }

  return { state, edgeState, apply, drop, blocks, areas, blockProps };
}
