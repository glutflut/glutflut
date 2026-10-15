// Moving fire and water on held streets: a WebGL layer over the glow. The
// motion shows the terrain: fire always runs uphill, water always downhill,
// the faster the steeper; flat streets barely move; fronts spark in place. Geometry is rebuilt at most a few times a second; per frame
// only the time uniform changes. Frames are capped (FPS) and stop when nothing
// held is in view, the tab is hidden or the map is zoomed out.
import type * as maplibregl from "maplibre-gl";
import type { TileLayer } from "./tiles";
import { interleaved, LOOKS, program, uniforms } from "./streetfx";

const FPS = 24;
const MIN_ZOOM = 14;
/** Pattern length on screen, px; keeps the look the same at every zoom. */
const WAVE_PX = 22;
const FLOATS = 9; // pos 2, normal 2, side, dist, kind, strength, speed

const VS = `#version 300 es
in vec2 a_pos;
in vec2 a_nrm;
in float a_side;
in float a_dist;
in float a_kind;
in float a_str;
in float a_speed;
uniform mat4 u_matrix;
uniform float u_halfw;
uniform float u_wave_m;
out float v_side;
out float v_u;
out float v_kind;
out float v_str;
out float v_speed;
void main() {
  gl_Position = u_matrix * vec4(a_pos + a_nrm * a_side * u_halfw, 0.0, 1.0);
  v_side = a_side;
  v_u = a_dist / u_wave_m;
  v_kind = a_kind;
  v_str = a_str;
  v_speed = a_speed;
}`;

const FS = `#version 300 es
precision highp float;
in float v_side;
in float v_u;
in float v_kind;
in float v_str;
in float v_speed;
uniform float u_time;
out vec4 frag;
${LOOKS}
void main() {
  float t = u_time * v_speed;
  vec3 c;
  float i;
  if (v_kind < 1.5) fireLook(v_u, v_side, t, c, i);
  else if (v_kind < 2.5) waterLook(v_u, v_side, t, 0.25, c, i);
  else frontLook(v_u, v_side, u_time, c, i);
  if (v_kind < 2.5) chevrons(v_u, v_side, t, 0.55, c, i);
  float a = i * (0.45 + 0.55 * v_str);
  frag = vec4(c * a, a);
}`;

type Flow = { o: number; s: number };

export class FlowLayer implements maplibregl.CustomLayerInterface {
  readonly id = "flow";
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
  private active = new Map<number, Flow>();
  private dirty = false;
  private building = 0;
  private timer = 0;
  private origin: [number, number] = [0, 0];

  constructor(private tiles: TileLayer) {}

  /** Held edge state (owner code, strength). */
  set(id: number, o: number, s: number) {
    if (o === 0) {
      if (!this.active.delete(id)) return;
    } else {
      this.active.set(id, { o, s });
    }
    this.markDirty();
  }

  private markDirty() {
    this.dirty = true;
    if (!this.building) this.building = window.setTimeout(() => this.rebuild(), 300);
  }

  onAdd(map: maplibregl.Map, gl: WebGL2RenderingContext) {
    this.map = map;
    this.gl = gl;
    const c = map.getCenter();
    this.origin = merc(c.lng, c.lat);
    this.prog = program(gl, VS, FS);
    this.u = uniforms(gl, this.prog, ["u_matrix", "u_halfw", "u_wave_m", "u_time"]);
    const attrs: [string, number][] = [["a_pos", 2], ["a_nrm", 2], ["a_side", 1], ["a_dist", 1], ["a_kind", 1], ["a_str", 1], ["a_speed", 1]];
    ({ vao: this.vao, vbo: this.vbo, ibo: this.ibo } = interleaved(gl, this.prog, attrs));
    document.addEventListener("visibilitychange", () => this.schedule());
    map.on("moveend", () => this.schedule());
    this.markDirty();
  }

  onRemove() {
    clearTimeout(this.timer);
    clearTimeout(this.building);
  }

  private rebuild() {
    this.building = 0;
    if (!this.dirty || !this.gl) return;
    this.dirty = false;
    const { edgeLine, edgeById, nodeElev } = this.tiles;
    const v: number[] = [];
    const idx: number[] = [];
    const [ox, oy] = this.origin;
    for (const [id, f] of this.active) {
      const line = edgeLine.get(id);
      const e = edgeById.get(id);
      if (!line || !e) {
        this.active.delete(id); // tile dropped
        continue;
      }
      // Metres along the line, source to target.
      const pts: [number, number, number][] = [];
      let d = 0;
      for (let i = 0; i + 1 < line.length; i += 2) {
        const [x, y] = merc(line[i], line[i + 1]);
        if (pts.length) {
          const [px, py] = pts[pts.length - 1];
          const k = 40_075_016.686 * Math.cos((line[i + 1] * Math.PI) / 180);
          d += Math.hypot(x - px, y - py) * k;
        }
        pts.push([x, y, d]);
      }
      if (pts.length < 2 || d < 0.5) continue;
      const dh = (nodeElev.get(e.target) ?? 0) - (nodeElev.get(e.source) ?? 0);
      // +1: flows source -> target. Fire up, water down; speed from the grade
      // (flat ~0.35, 5 % ~1, 17 %+ at the cap).
      const dir = f.o === 3 ? 1 : (f.o === 1 ? dh >= 0 : dh <= 0) ? 1 : -1;
      const speed = f.o === 3 ? 1 : clamp(0.35 + 12.5 * Math.abs(dh / d), 0.35, 2.5);
      for (let i = 0; i + 1 < pts.length; i++) {
        const [x0, y0, d0] = pts[i];
        const [x1, y1, d1] = pts[i + 1];
        const len = Math.hypot(x1 - x0, y1 - y0);
        if (len === 0) continue;
        const nx = -(y1 - y0) / len;
        const ny = (x1 - x0) / len;
        const a = dir > 0 ? d0 : d - d0;
        const b = dir > 0 ? d1 : d - d1;
        const base = v.length / FLOATS;
        for (const [x, y, dd] of [[x0, y0, a], [x1, y1, b]] as const) {
          for (const side of [-1, 1]) v.push(x - ox, y - oy, nx, ny, side, dd, f.o, Math.min(1, f.s), speed);
        }
        idx.push(base, base + 1, base + 2, base + 1, base + 3, base + 2);
      }
    }
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.DYNAMIC_DRAW);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, this.ibo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint32Array(idx), gl.DYNAMIC_DRAW);
    this.count = idx.length;
    this.map.triggerRepaint();
  }

  /** Next frame while there is something to animate. */
  /** No frames while the map is covered (camera view). */
  paused = false;

  pause(on: boolean) {
    this.paused = on;
    if (!on) this.schedule();
  }

  private schedule() {
    if (this.timer || this.paused || document.hidden || !this.count || this.map.getZoom() < MIN_ZOOM) return;
    this.timer = window.setTimeout(() => {
      this.timer = 0;
      this.map.triggerRepaint();
    }, 1000 / FPS);
  }

  render(gl: WebGL2RenderingContext, args: maplibregl.CustomRenderMethodInput) {
    const zoom = this.map.getZoom();
    if (!this.count || zoom < MIN_ZOOM) return;
    const lat = this.map.getCenter().lat;
    const worldPx = 512 * 2 ** zoom;
    const mpp = (40_075_016.686 * Math.cos((lat * Math.PI) / 180)) / worldPx;
    // Width like the glow layer: ~7 px at z14 to ~22 px at z18.
    const widthPx = clamp(7 + (zoom - 14) * 3.75, 5, 26);
    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(this.u.u_matrix, false, translated(args.defaultProjectionData.mainMatrix as ArrayLike<number>, this.origin));
    gl.uniform1f(this.u.u_halfw, widthPx / 2 / worldPx);
    gl.uniform1f(this.u.u_wave_m, WAVE_PX * mpp);
    gl.uniform1f(this.u.u_time, (performance.now() / 1000) % 3600);
    gl.bindVertexArray(this.vao);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE); // additive: light on a dark map
    gl.drawElements(gl.TRIANGLES, this.count, gl.UNSIGNED_INT, 0);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    gl.bindVertexArray(null);
    this.schedule();
  }
}

function clamp(v: number, lo: number, hi: number) {
  return Math.min(hi, Math.max(lo, v));
}

/** Web mercator 0..1. */
function merc(lon: number, lat: number): [number, number] {
  const r = (lat * Math.PI) / 180;
  return [(lon + 180) / 360, (1 - Math.log(Math.tan(Math.PI / 4 + r / 2)) / Math.PI) / 2];
}

/** `m` · translate(origin), in doubles; vertices are relative to the origin for float32 precision. */
function translated(m: ArrayLike<number>, [x, y]: [number, number]): Float32Array {
  const out = new Float32Array(16);
  for (let i = 0; i < 16; i++) out[i] = m[i];
  for (let r = 0; r < 4; r++) out[12 + r] = m[r] * x + m[4 + r] * y + m[12 + r];
  return out;
}
