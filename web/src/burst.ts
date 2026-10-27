// The moment of a claim: the element shoots out along the streets from the
// junction, over the real terrain. Fire runs fast uphill and slow downhill,
// water the other way, so every place looks different. A new permanent source
// sends one big wave through its supply radius and lights a column on the spot.
// One-off WebGL layer: geometry once per burst, frames only while one runs.
import * as maplibregl from "maplibre-gl";
import type { Faction } from "./net";
import { interleaved, LOOKS, program, uniforms } from "./streetfx";
import type { TileLayer } from "./tiles";

export type BurstKind = "claim" | "first" | "keep";

/** Reach in cost-metres (a flat street costs its length) and front duration, s. */
const SHAPE: Record<BurstKind, { reach: number; dur: number }> = {
  claim: { reach: 150, dur: 1.3 },
  first: { reach: 280, dur: 2.0 },
  keep: { reach: 520, dur: 3.0 },
};
/** Glow after the front passed, s. */
const TAIL = 1.6;
const WAVE_PX = 22;
const FLOATS = 9; // pos 2, normal 2, side, cost, kind, t0, speed (cost-m/s)

const VS = `#version 300 es
in vec2 a_pos;
in vec2 a_nrm;
in float a_side;
in float a_cost;
in float a_kind;
in float a_t0;
in float a_speed;
uniform mat4 u_matrix;
uniform float u_halfw;
out float v_side;
out float v_cost;
out float v_kind;
out float v_t0;
out float v_speed;
void main() {
  gl_Position = u_matrix * vec4(a_pos + a_nrm * a_side * u_halfw, 0.0, 1.0);
  v_side = a_side;
  v_cost = a_cost;
  v_kind = a_kind;
  v_t0 = a_t0;
  v_speed = a_speed;
}`;

const FS = `#version 300 es
precision highp float;
in float v_side;
in float v_cost;
in float v_kind;
in float v_t0;
in float v_speed;
uniform float u_time;
uniform float u_wave_m;
uniform float u_tail;
out vec4 frag;
${LOOKS}
void main() {
  float age = u_time - v_t0;
  float since = age - v_cost / v_speed; // s since the front passed here
  if (since < 0.0) discard;
  float u = v_cost / u_wave_m;
  vec3 c;
  float i;
  if (v_kind < 1.5) fireLook(u, v_side, age * 2.2, c, i);
  else waterLook(u, v_side, age * 2.0, 0.35, c, i);
  float head = exp(-since * 4.0);
  float tail = 1.0 - smoothstep(0.0, u_tail, since);
  float core = 1.0 - abs(v_side);
  // White-hot head, the element's look behind it, fading out.
  c = mix(c, vec3(1.0, 0.98, 0.92), 0.85 * head * core);
  float a = clamp((0.35 + 0.65 * i + 1.4 * head) * core * tail, 0.0, 0.95);
  frag = vec4(c * a, a);
}`;

interface Burst {
  t0: number;
  end: number;
  verts: number[];
  idx: number[];
}

export class BurstLayer implements maplibregl.CustomLayerInterface {
  readonly id = "burst";
  readonly type = "custom" as const;
  readonly renderingMode = "2d" as const;
  private map!: maplibregl.Map;
  private gl!: WebGL2RenderingContext;
  private prog!: WebGLProgram;
  private vao!: WebGLVertexArrayObject;
  private vbo!: WebGLBuffer;
  private ibo!: WebGLBuffer;
  private u: Record<string, WebGLUniformLocation | null> = {};
  private count = 0;
  private bursts: Burst[] = [];
  private origin: [number, number] = [0, 0];

  constructor(private tiles: TileLayer) {}

  onAdd(map: maplibregl.Map, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    const c = map.getCenter();
    this.origin = merc(c.lng, c.lat);
    this.prog = program(gl, VS, FS);
    this.u = uniforms(gl, this.prog, ["u_matrix", "u_halfw", "u_wave_m", "u_time", "u_tail"]);
    const attrs: [string, number][] = [["a_pos", 2], ["a_nrm", 2], ["a_side", 1], ["a_cost", 1], ["a_kind", 1], ["a_t0", 1], ["a_speed", 1]];
    ({ vao: this.vao, vbo: this.vbo, ibo: this.ibo } = interleaved(gl, this.prog, attrs));
  }

  /** Start a burst at a junction. */
  fire(node: number, side: Faction, kind: BurstKind) {
    if (!this.gl) return;
    const { reach, dur } = SHAPE[kind];
    const cost = this.spread(node, side, reach);
    if (!cost.size) return;
    const t0 = now();
    const b: Burst = { t0, end: t0 + dur + TAIL + 0.2, verts: [], idx: [] };
    this.geometry(b, cost, side, reach, reach / dur);
    this.bursts.push(b);
    this.upload();
    this.map.triggerRepaint();
  }

  /** Cheapest arrival per node; walking with the element is cheap. */
  private spread(start: number, side: Faction, reach: number) {
    const { edgesOf, edgeById, edgeLine, nodeElev } = this.tiles;
    const cost = new Map<number, number>([[start, 0]]);
    const open: [number, number][] = [[0, start]];
    while (open.length) {
      // Small graphs (a few hundred nodes): a sorted pop is enough.
      let k = 0;
      for (let i = 1; i < open.length; i++) if (open[i][0] < open[k][0]) k = i;
      const [c, n] = open.splice(k, 1)[0];
      if (c > (cost.get(n) ?? Infinity)) continue;
      for (const id of edgesOf.get(n) ?? []) {
        const e = edgeById.get(id);
        const line = edgeLine.get(id);
        if (!e || !line) continue;
        const m = e.source === n ? e.target : e.source;
        const len = lineLength(line);
        const nc = c + len * factor(side, (nodeElev.get(m) ?? 0) - (nodeElev.get(n) ?? 0), len);
        if (c < reach && nc < (cost.get(m) ?? Infinity)) {
          cost.set(m, nc);
          open.push([nc, m]);
        }
      }
    }
    return cost;
  }

  private geometry(b: Burst, cost: Map<number, number>, side: Faction, reach: number, speed: number) {
    const { edgesOf, edgeById, edgeLine, nodeElev } = this.tiles;
    const [ox, oy] = this.origin;
    const kind = side === "fire" ? 1 : 2;
    const seen = new Set<number>();
    for (const n of cost.keys())
      for (const id of edgesOf.get(n) ?? []) {
        if (seen.has(id)) continue;
        seen.add(id);
        const e = edgeById.get(id);
        const line = edgeLine.get(id);
        if (!e || !line) continue;
        const cs = cost.get(e.source) ?? Infinity;
        const ct = cost.get(e.target) ?? Infinity;
        const len = lineLength(line);
        const dh = (nodeElev.get(e.target) ?? 0) - (nodeElev.get(e.source) ?? 0);
        const fs = factor(side, dh, len); // source -> target
        const ft = factor(side, -dh, len);
        const pts: [number, number, number][] = [];
        let d = 0;
        for (let i = 0; i + 1 < line.length; i += 2) {
          const [x, y] = merc(line[i], line[i + 1]);
          if (pts.length) {
            const [px, py] = pts[pts.length - 1];
            d += Math.hypot(x - px, y - py) * 40_075_016.686 * Math.cos((line[i + 1] * Math.PI) / 180);
          }
          pts.push([x, y, d]);
        }
        const scale = d > 0 ? len / d : 1;
        // Arrival along the edge: from whichever end gets there first, stop at the reach.
        const at = (dd: number) => Math.min(cs + dd * scale * fs, ct + (len - dd * scale) * ft);
        for (let i = 0; i + 1 < pts.length; i++) {
          const [x0, y0, d0] = pts[i];
          const [x1, y1, d1] = pts[i + 1];
          const a = at(d0);
          const c1 = at(d1);
          if (a > reach && c1 > reach) continue;
          const l = Math.hypot(x1 - x0, y1 - y0);
          if (l === 0) continue;
          const nx = -(y1 - y0) / l;
          const ny = (x1 - x0) / l;
          const base = b.verts.length / FLOATS;
          for (const [x, y, cc] of [[x0, y0, a], [x1, y1, c1]] as const)
            for (const s of [-1, 1]) b.verts.push(x - ox, y - oy, nx, ny, s, Math.min(cc, reach * 1.2), kind, b.t0, speed);
          b.idx.push(base, base + 1, base + 2, base + 1, base + 3, base + 2);
        }
      }
  }

  private upload() {
    const v: number[] = [];
    const idx: number[] = [];
    for (const b of this.bursts) {
      const off = v.length / FLOATS;
      for (const x of b.verts) v.push(x);
      for (const i of b.idx) idx.push(i + off);
    }
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.DYNAMIC_DRAW);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.ibo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint32Array(idx), gl.DYNAMIC_DRAW);
    this.count = idx.length;
  }

  render(gl: WebGL2RenderingContext, args: maplibregl.CustomRenderMethodInput) {
    if (!this.count) return;
    const t = now();
    if (this.bursts.some((b) => b.end < t)) {
      this.bursts = this.bursts.filter((b) => b.end >= t);
      this.upload();
      if (!this.count) return;
    }
    const zoom = this.map.getZoom();
    const lat = this.map.getCenter().lat;
    const worldPx = 512 * 2 ** zoom;
    const mpp = (40_075_016.686 * Math.cos((lat * Math.PI) / 180)) / worldPx;
    // A bit wider than the flow layer, so it reads over it.
    const widthPx = clamp(8 + (zoom - 14) * 4, 7, 26);
    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(this.u.u_matrix, false, translated(args.defaultProjectionData.mainMatrix as ArrayLike<number>, this.origin));
    gl.uniform1f(this.u.u_halfw, widthPx / 2 / worldPx);
    gl.uniform1f(this.u.u_wave_m, WAVE_PX * mpp);
    gl.uniform1f(this.u.u_time, t);
    gl.uniform1f(this.u.u_tail, TAIL);
    gl.bindVertexArray(this.vao);
    // Premultiplied "over", not additive: it has to read on the light map too.
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.drawElements(gl.TRIANGLES, this.count, gl.UNSIGNED_INT, 0);
    gl.bindVertexArray(null);
    requestAnimationFrame(() => this.map.triggerRepaint());
  }
}

/** Shock rings at the spot, and for a permanent source a column of embers or
 *  a growing crystal. DOM only, so it also runs where WebGL effects are off. */
export function spotBurst(map: maplibregl.Map, ll: [number, number], side: Faction, kind: BurstKind) {
  const el = document.createElement("div");
  el.className = `claim-burst ${side} ${kind}`;
  for (let i = 0; i < 2; i++) el.append(span("ring"));
  if (kind === "keep") {
    el.append(span("column"));
    if (side === "fire") for (let i = 0; i < 14; i++) el.append(ember(i));
    else el.append(span("crystal"));
  }
  const m = new maplibregl.Marker({ element: el }).setLngLat(ll).addTo(map);
  if (fx.slow > 1) for (const an of el.getAnimations({ subtree: true })) an.playbackRate = 1 / fx.slow;
  setTimeout(() => m.remove(), (kind === "keep" ? 3600 : 1800) * fx.slow);
}

function span(cls: string) {
  const s = document.createElement("span");
  s.className = cls;
  return s;
}

function ember(i: number) {
  const s = span("ember");
  s.style.setProperty("--x", `${Math.round((Math.random() - 0.5) * 50)}px`);
  s.style.animationDelay = `${(i * 0.13 + Math.random() * 0.2).toFixed(2)}s`;
  return s;
}

/** Cost per metre: with the element down to 0.35, against it up to 2.5. */
function factor(side: Faction, dh: number, len: number) {
  const g = len > 0.5 ? dh / len : 0;
  return clamp(1 - 7 * (side === "fire" ? g : -g), 0.35, 2.5);
}

function lineLength(line: number[]) {
  let d = 0;
  for (let i = 2; i + 1 < line.length; i += 2) {
    const lat = ((line[i + 1] + line[i - 1]) / 2) * (Math.PI / 180);
    d += Math.hypot((line[i] - line[i - 2]) * Math.cos(lat), line[i + 1] - line[i - 1]) * 111_320;
  }
  return d;
}

/** Slow motion for trying it out (`?fx=4`, main.ts). */
export const fx = { slow: 1 };
const now = () => (performance.now() / 1000 / fx.slow) % 3600;

function clamp(v: number, lo: number, hi: number) {
  return Math.min(hi, Math.max(lo, v));
}

function merc(lon: number, lat: number): [number, number] {
  const r = (lat * Math.PI) / 180;
  return [(lon + 180) / 360, (1 - Math.log(Math.tan(Math.PI / 4 + r / 2)) / Math.PI) / 2];
}

function translated(m: ArrayLike<number>, [x, y]: [number, number]): Float32Array {
  const out = new Float32Array(16);
  for (let i = 0; i < 16; i++) out[i] = m[i];
  for (let r = 0; r < 4; r++) out[12 + r] = m[r] * x + m[4 + r] * y + m[12 + r];
  return out;
}
