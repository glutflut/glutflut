// The street graph in web-mercator tiles (server: crates/server/src/tiles.rs).
// Loads the tiles in view and around the player, drops the least recently
// used ones beyond a budget, and tells the server which ones it holds.
import type * as maplibregl from "maplibre-gl";
import { degPerM } from "./geo";
import { withRegion } from "./region";

export interface Meta {
  region: { slug: string; name: string; centroid: [number, number]; bounds: [number, number, number, number] };
  nodes: number;
  /** `v`: graph version, part of tile URLs so a new graph bypasses cached tiles. */
  tiles: { z: number; index: [number, number, number][]; v?: string };
  hotspot: [number, number] | null;
  /** Blocks on: the share of a block's border a side must hold (server `[area] share`). */
  area?: number | null;
}

type TileData = {
  n: [number, number, number, number?][];
  e: [number, number, number, number[]][];
  /** Blocks: id, border junctions, rings (flat [lon, lat, ..]; the first is the outline). */
  b?: [number, number[], number[][]][];
  /** Natural sources of the tile's own junctions: id, kind (0 fire, 1 water, 2 spring). */
  s?: [number, 0 | 1 | 2][];
};
export type Edge = { id: number; source: number; target: number };

/** Nodes in view above which the map shows the overview instead. */
export const VIEW_BUDGET = 20_000;
/** Nodes kept loaded (view, player, recently seen). */
const KEEP_BUDGET = 35_000;
/** Tiles around the player within this distance are always held. */
const AROUND_M = 150;

export function lonToX(lon: number, z: number) {
  return Math.floor(((lon + 180) / 360) * 2 ** z);
}
export function latToY(lat: number, z: number) {
  const r = (lat * Math.PI) / 180;
  return Math.floor(((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * 2 ** z);
}
export function xToLon(x: number, z: number) {
  return (x / 2 ** z) * 360 - 180;
}
export function yToLat(y: number, z: number) {
  const n = Math.PI - (2 * Math.PI * y) / 2 ** z;
  return (180 / Math.PI) * Math.atan(Math.sinh(n));
}

interface Held {
  nodes: number[];
  edges: Edge[];
  blocks: number[];
  natural: number[];
  count: number;
  used: number;
}

export class TileLayer {
  readonly z: number;
  private readonly v: string;
  /** "x,y" -> nodes in the tile (the region's tiles only). */
  private index = new Map<string, number>();
  private held = new Map<string, Held>();
  private loading = new Set<string>();
  private refs = new Map<number, number>();
  readonly nodePos = new Map<number, [number, number]>();
  /** Node height in metres (0 from an older server). */
  readonly nodeElev = new Map<number, number>();
  /** Edge polyline, flat [lon, lat, lon, lat, ..]. */
  readonly edgeLine = new Map<number, number[]>();
  readonly edgeById = new Map<number, Edge>();
  readonly edgesOf = new Map<number, Set<number>>();
  /** Block -> its border junctions; junction -> the blocks it borders. */
  readonly blockNodes = new Map<number, number[]>();
  readonly blocksOf = new Map<number, Set<number>>();
  /** Blocks added (their owners need painting). */
  onBlocks: (ids: number[]) => void = () => {};
  /** Properties a block gets when its tile loads. */
  blockProps: (id: number) => Record<string, unknown> = () => ({});
  /** Whether the view needs more than the budget (map shows the overview). */
  tooMany = false;
  private around: string[] = [];
  private busy = false;
  private again = false;

  private viewBudget: number;
  private keepBudget: number;
  private withNodes: boolean;

  constructor(
    private map: maplibregl.Map,
    meta: Meta,
    /** Tiles added: their new node ids. */
    private onAdd: (nodes: number[]) => void,
    /** Nodes dropped with their tile. */
    private onDrop: (nodes: number[]) => void,
    /** The set of held tiles changed. */
    private onView: (tiles: [number, number][]) => void,
    /** Budgets (a desktop holds more) and whether there is a "nodes" source. */
    opts: { view?: number; keep?: number; nodes?: boolean } = {},
  ) {
    this.viewBudget = opts.view ?? VIEW_BUDGET;
    this.keepBudget = opts.keep ?? KEEP_BUDGET;
    this.withNodes = opts.nodes ?? true;
    this.z = meta.tiles.z;
    this.v = meta.tiles.v ?? "";
    for (const [x, y, n] of meta.tiles.index) this.index.set(`${x},${y}`, n);
  }

  /** Nodes in the tiles covering this box (without enumerating huge boxes). */
  private covering(w: number, s: number, e: number, n: number, limit: number): { keys: string[]; nodes: number } {
    const [x0, x1] = [lonToX(w, this.z), lonToX(e, this.z)];
    const [y0, y1] = [latToY(n, this.z), latToY(s, this.z)];
    if ((x1 - x0 + 1) * (y1 - y0 + 1) > 4 * this.index.size) {
      // Wider than the region: every tile of it.
      let nodes = 0;
      for (const c of this.index.values()) nodes += c;
      return nodes > limit ? { keys: [], nodes } : { keys: [...this.index.keys()], nodes };
    }
    const keys: string[] = [];
    let nodes = 0;
    for (let x = x0; x <= x1; x++)
      for (let y = y0; y <= y1; y++) {
        const k = `${x},${y}`;
        const c = this.index.get(k);
        if (c === undefined) continue;
        keys.push(k);
        nodes += c;
        if (nodes > limit) return { keys: [], nodes };
      }
    return { keys, nodes };
  }

  /** Tiles near the player are held whatever the view (claims, ratings). */
  setPlayer(p: [number, number]) {
    const [kx, ky] = degPerM(p[1]);
    const dx = AROUND_M * kx, dy = AROUND_M * ky;
    const keys = this.covering(p[0] - dx, p[1] - dy, p[0] + dx, p[1] + dy, Infinity).keys;
    if (keys.join(";") === this.around.join(";")) return;
    this.around = keys;
    this.refresh();
  }

  /** Load what the view and the player need; call on moveend. */
  async refresh() {
    if (this.busy) return void (this.again = true);
    this.busy = true;
    try {
      do {
        this.again = false;
        await this.step();
      } while (this.again);
    } finally {
      this.busy = false;
    }
  }

  private async step() {
    const b = this.map.getBounds();
    const view = this.covering(b.getWest(), b.getSouth(), b.getEast(), b.getNorth(), this.viewBudget);
    this.tooMany = view.nodes > this.viewBudget;
    const want = [...new Set([...view.keys, ...this.around])];
    const now = performance.now();
    for (const k of want) {
      const h = this.held.get(k);
      if (h) h.used = now;
    }
    const missing = want.filter((k) => !this.held.has(k) && !this.loading.has(k));
    let changed = false;
    // A few at a time; each tile shows as soon as it's there.
    for (let i = 0; i < missing.length; i += 6) {
      const batch = missing.slice(i, i + 6);
      for (const k of batch) this.loading.add(k);
      const got = await Promise.all(
        batch.map(async (k) => {
          try {
            const res = await fetch(withRegion(`/api/tile/${k.replace(",", "/")}${this.v ? `?v=${this.v}` : ""}`));
            return res.ok ? ([k, (await res.json()) as TileData] as const) : null;
          } catch {
            return null;
          } finally {
            this.loading.delete(k);
          }
        }),
      );
      const ok = got.filter((g) => g !== null);
      if (ok.length) {
        this.add(ok, now);
        changed = true;
      }
    }
    if (this.evict(new Set(want))) changed = true;
    if (changed) this.onView([...this.held.keys()].map((k) => k.split(",").map(Number) as [number, number]));
  }

  private add(tiles: (readonly [string, TileData])[], now: number) {
    const nodes: GeoJSON.Feature[] = [];
    const edges: GeoJSON.Feature[] = [];
    const blocks: GeoJSON.Feature[] = [];
    const natural: GeoJSON.Feature[] = [];
    const fresh: number[] = [];
    for (const [k, t] of tiles) {
      const held: Held = { nodes: [], edges: [], blocks: [], natural: [], count: t.n.length, used: now };
      for (const [id, lon, lat, elev] of t.n) {
        held.nodes.push(id);
        const r = this.refs.get(id) ?? 0;
        this.refs.set(id, r + 1);
        if (r) continue;
        this.nodePos.set(id, [lon, lat]);
        this.nodeElev.set(id, elev ?? 0);
        fresh.push(id);
        nodes.push({ type: "Feature", id, properties: { id }, geometry: { type: "Point", coordinates: [lon, lat] } });
      }
      for (const [id, source, target, flat] of t.e) {
        const e = { id, source, target };
        held.edges.push(e);
        this.edgeById.set(id, e);
        this.edgeLine.set(id, flat);
        for (const n of [source, target]) {
          if (!this.edgesOf.has(n)) this.edgesOf.set(n, new Set());
          this.edgesOf.get(n)!.add(id);
        }
        const coords: [number, number][] = [];
        for (let i = 0; i + 1 < flat.length; i += 2) coords.push([flat[i], flat[i + 1]]);
        edges.push({ type: "Feature", id, properties: e, geometry: { type: "LineString", coordinates: coords } });
      }
      for (const [id, border, rings] of t.b ?? []) {
        held.blocks.push(id);
        this.blockNodes.set(id, border);
        for (const n of border) {
          if (!this.blocksOf.has(n)) this.blocksOf.set(n, new Set());
          this.blocksOf.get(n)!.add(id);
        }
        const coordinates = rings.map((flat) => {
          const ring: [number, number][] = [];
          for (let i = 0; i + 1 < flat.length; i += 2) ring.push([flat[i], flat[i + 1]]);
          return ring;
        });
        blocks.push({ type: "Feature", id, properties: this.blockProps(id), geometry: { type: "Polygon", coordinates } });
      }
      for (const [id, kind] of t.s ?? []) {
        const at = this.nodePos.get(id);
        if (!at) continue;
        held.natural.push(id);
        natural.push({ type: "Feature", id, properties: { k: kind }, geometry: { type: "Point", coordinates: at } });
      }
      this.held.set(k, held);
    }
    const naturalSource = this.map.getSource("natural") as maplibregl.GeoJSONSource | undefined;
    if (natural.length && naturalSource) naturalSource.updateData({ add: natural });
    const blockSource = this.map.getSource("blocks") as maplibregl.GeoJSONSource | undefined;
    if (blocks.length && blockSource) blockSource.updateData({ add: blocks });
    if (this.withNodes) (this.map.getSource("nodes") as maplibregl.GeoJSONSource).updateData({ add: nodes });
    (this.map.getSource("edges") as maplibregl.GeoJSONSource).updateData({ add: edges });
    this.onAdd(fresh);
    if (blocks.length) this.onBlocks(blocks.map((f) => f.id as number));
  }

  /** Drop least recently used tiles outside `keep` while over budget. */
  private evict(keep: Set<string>): boolean {
    let total = 0;
    for (const h of this.held.values()) total += h.count;
    if (total <= this.keepBudget) return false;
    const old = [...this.held.entries()].filter(([k]) => !keep.has(k)).sort((a, b) => a[1].used - b[1].used);
    const goneNodes: number[] = [];
    const goneEdges: number[] = [];
    const goneBlocks: number[] = [];
    const goneNatural: number[] = [];
    for (const [k, h] of old) {
      if (total <= this.keepBudget) break;
      total -= h.count;
      this.held.delete(k);
      goneNatural.push(...h.natural);
      for (const b of h.blocks) {
        for (const n of this.blockNodes.get(b) ?? []) this.blocksOf.get(n)?.delete(b);
        this.blockNodes.delete(b);
        goneBlocks.push(b);
      }
      for (const e of h.edges) {
        this.edgeById.delete(e.id);
        this.edgeLine.delete(e.id);
        goneEdges.push(e.id);
        for (const n of [e.source, e.target]) this.edgesOf.get(n)?.delete(e.id);
      }
      for (const id of h.nodes) {
        const r = (this.refs.get(id) ?? 1) - 1;
        if (r > 0) {
          this.refs.set(id, r);
          continue;
        }
        this.refs.delete(id);
        this.nodePos.delete(id);
        this.nodeElev.delete(id);
        this.edgesOf.delete(id);
        this.blocksOf.delete(id);
        goneNodes.push(id);
      }
    }
    if (!goneNodes.length && !goneEdges.length) return false;
    if (this.withNodes) (this.map.getSource("nodes") as maplibregl.GeoJSONSource).updateData({ remove: goneNodes });
    (this.map.getSource("edges") as maplibregl.GeoJSONSource).updateData({ remove: goneEdges });
    const blockSource = this.map.getSource("blocks") as maplibregl.GeoJSONSource | undefined;
    if (goneBlocks.length && blockSource) blockSource.updateData({ remove: goneBlocks });
    const naturalSource = this.map.getSource("natural") as maplibregl.GeoJSONSource | undefined;
    if (goneNatural.length && naturalSource) naturalSource.updateData({ remove: goneNatural });
    this.onDrop(goneNodes);
    return true;
  }

  /** Streets of this tile are loaded. */
  holds(x: number, y: number) {
    return this.held.has(`${x},${y}`);
  }

  /** Tile outlines for the overview: `[x, y, ...values]` rows to polygons. */
  polygons(rows: number[][], props: (r: number[]) => Record<string, unknown>): GeoJSON.FeatureCollection {
    return {
      type: "FeatureCollection",
      features: rows.map((r) => {
        const [x, y] = r;
        const w = xToLon(x, this.z), e = xToLon(x + 1, this.z);
        const n = yToLat(y, this.z), s = yToLat(y + 1, this.z);
        return {
          type: "Feature",
          properties: props(r),
          geometry: { type: "Polygon", coordinates: [[[w, s], [e, s], [e, n], [w, n], [w, s]]] },
        };
      }),
    };
  }
}

/** Region settings and tile index; a region that isn't loaded yet answers 503
 *  for a few seconds (up to a minute for a big city): `waiting` meanwhile. */
export async function loadMeta(waiting: () => void): Promise<Meta> {
  for (let i = 0; ; i++) {
    const res = await fetch(withRegion("/api/region/meta"));
    if (res.ok) return res.json();
    if (res.status !== 503 || i >= 90) throw new Error(`meta: ${res.status}`);
    waiting();
    await new Promise((r) => setTimeout(r, 2000));
  }
}
