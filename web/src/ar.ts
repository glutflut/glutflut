// Look through the camera: the streets around the player as fire and water on
// the ground, over the rear camera image. Orientation from the device sensors
// (3DoF), position from GPS; the heading can be nudged by swiping or snapped to
// the nearest street ("align"). The camera image never leaves the device.
// Loaded on demand. Raw WebGL2; camera maths in armath.ts.
import { cameraQuat, cylinder, depth, heading, linearRgb, mat3, perspective, qslerp, translated, viewMatrix, type Vec3 } from "./armath";
import { enu } from "./geo";
import { interleaved, LOOKS, program, uniforms } from "./streetfx";

/** A street near the player: flat [lon, lat, ..], owner code (0 neutral,
 *  1 fire, 2 water, 3 front), strength, heights at source and target. */
export type ArEdge = { line: number[]; o: number; s: number; h0: number; h1: number };
export type ArSource = { ll: [number, number]; faction: "fire" | "water"; building: boolean };

export interface ArContext {
  me(): [number, number] | null;
  edges(center: [number, number], r: number): ArEdge[];
  sources(): ArSource[];
  /** Ground height (m) near a point, for the eye level. */
  ground(at: [number, number]): number;
  text(key: "hint" | "align" | "nocamera" | "nocompass" | "close" | "nearest", vars?: Record<string, string | number>): string;
}

const RADIUS_M = 70;
const EYE_M = 1.5;
/** Rebuild the scene when the player moved this far from its origin. */
const REBUILD_M = 20;
/** Smoothing (s): the view follows the sensors this fast (compass and gyro
 *  noise), the eye glides to each GPS fix instead of jumping by its error. */
const TURN_S = 0.12;
const GLIDE_S = 1.5;
/** A fix this far off is a real move (or a fresh fix): jump there. */
const SNAP_M = 25;
const COLORS = { fire: 0xff7a1a, water: 0x2f9bff };

const VS = /* glsl */ `#version 300 es
in vec3 a_pos;
in float a_side;
in float a_dist;
in float a_kind;
in float a_str;
in float a_speed;
in float a_up;
uniform mat4 u_proj;
uniform mat4 u_view;
out float v_side;
out float v_dist;
out float v_kind;
out float v_str;
out float v_speed;
out float v_up;
out vec3 v_pos;
void main() {
  v_side = a_side; v_dist = a_dist; v_kind = a_kind; v_str = a_str; v_speed = a_speed; v_up = a_up;
  v_pos = a_pos;
  gl_Position = u_proj * u_view * vec4(a_pos, 1.0);
}`;

// Same looks as the map's flow layer, in metres: fire tongues and a low wall of
// flames, water waves with foam, chevrons along the flow, sparks on fronts,
// a faint line on neutral streets; everything fades with distance.
const FS = /* glsl */ `#version 300 es
precision highp float;
uniform float u_time;
uniform vec3 u_cam;
in float v_side;
in float v_dist;
in float v_kind;
in float v_str;
in float v_speed;
in float v_up;
in vec3 v_pos;
out vec4 frag;
${LOOKS}
void main() {
  float side = abs(v_side);
  float t = u_time * v_speed;
  float u = v_dist / 1.8;
  vec3 c;
  float i;
  if (v_up > 0.0) {
    // Wall of flames above a fire street: tongues rising, thinning out upwards.
    float h = v_up;
    float n = noise(vec2(u * 1.6 - t * 0.8, h * 2.5 - u_time * 2.2));
    float n2 = noise(vec2(u * 3.7, h * 5.0 - u_time * 3.5));
    i = smoothstep(h * 1.1, h * 1.1 + 0.35, 0.55 * n + 0.45 * n2) * pow(1.0 - h, 1.3);
    c = mix(vec3(1.0, 0.85, 0.45), vec3(0.9, 0.18, 0.02), h);
  } else if (v_kind < 0.5) {
    // Neutral street: two edge lines, visible in daylight, for orientation.
    i = 0.55 * (1.0 - smoothstep(0.0, 0.07, abs(side - 0.88)));
    c = vec3(0.92, 0.95, 1.0);
  } else if (v_kind < 1.5) fireLook(u, v_side, t, c, i);
  else if (v_kind < 2.5) waterLook(u, v_side, t, 0.3, c, i);
  else frontLook(u, v_side, u_time, c, i);
  if (v_up == 0.0 && v_kind > 0.5 && v_kind < 2.5) chevrons(u, v_side, t, 0.5, c, i);
  float d = distance(v_pos.xz, u_cam.xz);
  // The wall of the street underfoot would stand in the camera: fade it close by.
  if (v_up > 0.0) i *= smoothstep(3.0, 9.0, d);
  // Denser than on the map: it has to hold up against daylight.
  float a = clamp(i * 1.5, 0.0, 0.95) * (v_kind < 0.5 ? 1.0 : 0.6 + 0.4 * v_str) * (1.0 - smoothstep(RADIUS - 25.0, RADIUS, d));
  frag = vec4(c * a, a);
}`.replace(/RADIUS/g, RADIUS_M.toFixed(1));

type Local = (ll: [number, number]) => [number, number];

// Light column over a claimed source: slim, bright in the middle of its
// silhouette, fading upwards.
const BEAM_VS = /* glsl */ `#version 300 es
in vec3 a_pos;
in vec3 a_nrm;
in vec2 a_uv;
uniform mat4 u_proj;
uniform mat4 u_mv;
uniform mat3 u_nrm;
out float v_h;
out float v_rim;
void main() {
  v_h = a_uv.y;
  vec3 n = normalize(u_nrm * a_nrm);
  vec4 p = u_mv * vec4(a_pos, 1.0);
  v_rim = abs(dot(n, normalize(-p.xyz)));
  gl_Position = u_proj * p;
}`;
const BEAM_FS = /* glsl */ `#version 300 es
precision highp float;
uniform vec3 u_color;
uniform float u_alpha;
in float v_h;
in float v_rim;
out vec4 frag;
void main() {
  float a = u_alpha * pow(1.0 - v_h, 1.6) * pow(v_rim, 1.5);
  vec3 c = mix(u_color, vec3(1.0), 0.35 * v_rim);
  frag = vec4(c * a, a);
}`;
const BEAM_H = 9;

/** Ribbon on the ground (and for fire a wall of flames) per street:
 *  interleaved pos 3, side, dist, kind, str, speed, up; plus the centre of
 *  its bounding box (draw order). */
const STREET_FLOATS = 9;
function streetGeometry(edges: ArEdge[], local: Local, ground: number) {
  const v: number[] = [];
  const idx: number[] = [];
  const vertex = (x: number, y: number, z: number, side: number, dist: number, e: ArEdge, speed: number, up: number) =>
    v.push(x, y, z, side, dist, e.o, Math.min(1, e.s), speed, up);
  for (const e of edges) {
    const pts: [number, number, number][] = [];
    let len = 0;
    for (let i = 0; i + 1 < e.line.length; i += 2) {
      const [x, z] = local([e.line[i], e.line[i + 1]]);
      if (pts.length) len += Math.hypot(x - pts[pts.length - 1][0], z - pts[pts.length - 1][1]);
      pts.push([x, z, len]);
    }
    if (pts.length < 2 || len < 0.5) continue;
    const dh = e.h1 - e.h0;
    // Fire runs uphill, water downhill; speed from the grade (as on the map).
    const dir = e.o === 1 ? (dh >= 0 ? 1 : -1) : e.o === 2 ? (dh <= 0 ? 1 : -1) : 1;
    const speed = e.o === 1 || e.o === 2 ? Math.min(2.5, 0.35 + 12.5 * Math.abs(dh / len)) : 1;
    const half = e.o === 0 ? 1.2 : 1.6;
    for (let i = 0; i + 1 < pts.length; i++) {
      const [x0, z0, d0] = pts[i];
      const [x1, z1, d1] = pts[i + 1];
      const l = Math.hypot(x1 - x0, z1 - z0);
      if (l === 0) continue;
      const nx = -(z1 - z0) / l;
      const nz = (x1 - x0) / l;
      const a = dir > 0 ? d0 : len - d0;
      const b = dir > 0 ? d1 : len - d1;
      // Street height along the edge, relative to the player's ground.
      const y0 = e.h0 + (dh * d0) / len - ground + 0.05;
      const y1 = e.h0 + (dh * d1) / len - ground + 0.05;
      let base = v.length / STREET_FLOATS;
      for (const [x, y, z, d] of [[x0, y0, z0, a], [x1, y1, z1, b]]) {
        for (const side of [-1, 1]) vertex(x + nx * side * half, y, z + nz * side * half, side, d, e, speed, 0);
      }
      idx.push(base, base + 1, base + 2, base + 1, base + 3, base + 2);
      if (e.o === 1) {
        // Wall of flames along the middle, 1.4 m high.
        base = v.length / STREET_FLOATS;
        for (const [x, y, z, d] of [[x0, y0, z0, a], [x1, y1, z1, b]]) {
          vertex(x, y, z, 0, d, e, speed, 0.001);
          vertex(x, y + 1.4, z, 0, d, e, speed, 1);
        }
        idx.push(base, base + 1, base + 2, base + 1, base + 3, base + 2);
      }
    }
  }
  const lo = [Infinity, Infinity, Infinity];
  const hi = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < v.length; i += STREET_FLOATS)
    for (let k = 0; k < 3; k++) (lo[k] = Math.min(lo[k], v[i + k])), (hi[k] = Math.max(hi[k], v[i + k]));
  const center: Vec3 = v.length ? [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2] : [0, 0, 0];
  return { v: new Float32Array(v), idx: new Uint32Array(idx), center };
}

function wrap(a: number) {
  return Math.atan2(Math.sin(a), Math.cos(a));
}

export async function openAr(host: HTMLElement, ctx: ArContext): Promise<() => void> {
  host.hidden = false;
  host.innerHTML =
    `<video playsinline muted autoplay></video><canvas></canvas>` +
    `<div class="ar-bar"><span class="ar-status"></span><button class="ar-align"></button></div>` +
    `<canvas class="ar-radar" width="240" height="240"></canvas>` +
    `<div class="ar-hint"></div>`;
  const video = host.querySelector("video")!;
  const canvas = host.querySelector("canvas")!;
  const status = host.querySelector<HTMLElement>(".ar-status")!;
  const alignBtn = host.querySelector<HTMLButtonElement>(".ar-align")!;
  const hint = host.querySelector<HTMLElement>(".ar-hint")!;
  alignBtn.textContent = ctx.text("align");
  hint.textContent = ctx.text("hint");
  setTimeout(() => (hint.hidden = true), 7000);

  // Camera: rear, no audio; never recorded or sent.
  let stream: MediaStream | null = null;
  let noCamera = false;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: { ideal: "environment" } }, audio: false });
    video.srcObject = stream;
    await video.play().catch(() => {});
  } catch {
    noCamera = true;
  }

  // Sensors. iOS asks for permission (inside the tap that opened AR).
  type IosDOE = typeof DeviceOrientationEvent & { requestPermission?: () => Promise<string> };
  await (DeviceOrientationEvent as IosDOE).requestPermission?.().catch(() => "denied");
  const orient = { alpha: 0, beta: 80, gamma: 0, have: false, absolute: false };
  const onOrient = (e: DeviceOrientationEvent & { webkitCompassHeading?: number }) => {
    if (e.beta === null || e.gamma === null) return;
    if (e.webkitCompassHeading !== undefined) {
      orient.alpha = 360 - e.webkitCompassHeading; // iOS: heading from north
      orient.absolute = true;
    } else if (e.alpha !== null && (e.absolute || !orient.absolute)) {
      orient.alpha = e.alpha;
      orient.absolute ||= e.absolute;
    }
    orient.beta = e.beta;
    orient.gamma = e.gamma;
    orient.have = true;
  };
  addEventListener("deviceorientationabsolute", onOrient as EventListener);
  addEventListener("deviceorientation", onOrient as EventListener);
  let nearest: number | null = null;
  const showStatus = () => {
    const notes = [
      noCamera && ctx.text("nocamera"),
      !orient.absolute && ctx.text("nocompass"),
      nearest !== null && ctx.text("nearest", { m: Math.round(nearest) }),
    ];
    status.textContent = notes.filter(Boolean).join(" · ");
  };

  // Swipe sideways to turn the world (compass error), or without sensors to look around.
  let offset = 0;
  let dragX: number | null = null;
  canvas.addEventListener("pointerdown", (e) => (dragX = e.clientX));
  canvas.addEventListener("pointermove", (e) => {
    if (dragX === null) return;
    offset -= (e.clientX - dragX) * 0.005;
    dragX = e.clientX;
  });
  for (const ev of ["pointerup", "pointercancel", "pointerleave"]) canvas.addEventListener(ev, () => (dragX = null));

  // Premultiplied alpha over the video (the canvas sits above it); no depth:
  // nothing writes it, draws go back to front.
  const gl = canvas.getContext("webgl2", { alpha: true, antialias: true, depth: false, premultipliedAlpha: true })!;
  const pixelRatio = Math.min(devicePixelRatio, 2);
  const street = program(gl, VS, FS);
  const su = uniforms(gl, street, ["u_proj", "u_view", "u_time", "u_cam"]);
  const sb = interleaved(gl, street, [["a_pos", 3], ["a_side", 1], ["a_dist", 1], ["a_kind", 1], ["a_str", 1], ["a_speed", 1], ["a_up", 1]]);
  let streetCount = 0;
  let streetCenter: Vec3 = [0, 0, 0];
  const beam = program(gl, BEAM_VS, BEAM_FS);
  const bu = uniforms(gl, beam, ["u_proj", "u_mv", "u_nrm", "u_color", "u_alpha"]);
  const bb = interleaved(gl, beam, [["a_pos", 3], ["a_nrm", 3], ["a_uv", 2]]);
  const cyl = cylinder(0.35, BEAM_H, 24);
  gl.bindBuffer(gl.ARRAY_BUFFER, bb.vbo);
  gl.bufferData(gl.ARRAY_BUFFER, cyl.v, gl.STATIC_DRAW);
  gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, bb.ibo);
  gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, cyl.idx, gl.STATIC_DRAW);
  type Beam = { at: Vec3; color: Vec3; alpha: number; building: boolean };
  let beams: Beam[] = [];
  const camera = { fov: 62, aspect: 1, proj: perspective(62, 1, 0.3, 400), q: cameraQuat(0, 80, 0, 0, 0), pos: [0, 0, 0] as Vec3, vp: [0, 0] };
  const COLOR_LIN = { fire: linearRgb(COLORS.fire), water: linearRgb(COLORS.water) };

  let origin: [number, number] | null = null;
  let ground = 0;
  /** Where the eye is, gliding towards the latest fix. */
  let eye: [number, number] | null = null;
  let lastFrame = 0;
  // x east, z south.
  const local: Local = (ll) => {
    const [e, n] = enu(ll, origin!);
    return [e, -n];
  };
  let lastEdges: ArEdge[] = [];
  const heightAt = (ll: [number, number]) => ctx.ground(ll);
  const rebuild = () => {
    const me = ctx.me();
    if (!me) return;
    if (!origin || Math.hypot(...local(me)) > REBUILD_M) {
      origin = me;
      ground = ctx.ground(me);
    }
    lastEdges = ctx.edges(me, RADIUS_M);
    nearest = nearestStreet(me)?.d ?? null;
    const g = streetGeometry(lastEdges, local, ground);
    gl.bindBuffer(gl.ARRAY_BUFFER, sb.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, g.v, gl.DYNAMIC_DRAW);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, sb.ibo);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, g.idx, gl.DYNAMIC_DRAW);
    streetCount = g.idx.length;
    streetCenter = g.center;
    showStatus();
    beams = [];
    for (const s of ctx.sources()) {
      const [x, z] = local(s.ll);
      if (Math.hypot(x, z) > RADIUS_M) continue;
      // Foot of the column; draws sort by its middle.
      beams.push({ at: [x, heightAt(s.ll) - ground, z], color: COLOR_LIN[s.faction], alpha: s.building ? 0.5 : 0.8, building: s.building });
    }
  };
  rebuild();
  const rebuildTimer = setInterval(rebuild, 1000);

  /** Nearest street segment: distance (m) and bearing (rad). */
  function nearestStreet(me: [number, number]) {
    const [px, pz] = local(me);
    let best: { d: number; b: number } | null = null;
    for (const e of lastEdges) {
      for (let i = 0; i + 3 < e.line.length; i += 2) {
        const [x0, z0] = local([e.line[i], e.line[i + 1]]);
        const [x1, z1] = local([e.line[i + 2], e.line[i + 3]]);
        const dx = x1 - x0;
        const dz = z1 - z0;
        const l2 = dx * dx + dz * dz || 1;
        const k = Math.max(0, Math.min(1, ((px - x0) * dx + (pz - z0) * dz) / l2));
        const d = Math.hypot(px - (x0 + k * dx), pz - (z0 + k * dz));
        if (!best || d < best.d) best = { d, b: Math.atan2(dx, -dz) };
      }
    }
    return best;
  }

  // Mini radar, view direction up: the streets within 40 m, you in the middle,
  // the camera's field of view as a cone and north on the rim.
  const radar = host.querySelector<HTMLCanvasElement>(".ar-radar")!;
  const rc = radar.getContext("2d")!;
  const RADAR_M = 40;
  const STROKE = ["rgba(230,236,245,0.55)", "#ff7a1a", "#2f9bff", "#d05cf0"];
  let lastRadar = 0;
  function drawRadar(view: number) {
    const me = eye ?? ctx.me();
    const W = radar.width;
    const c = W / 2;
    const k = (c - 8) / RADAR_M;
    rc.clearRect(0, 0, W, W);
    rc.save();
    rc.beginPath();
    rc.arc(c, c, c - 2, 0, 2 * Math.PI);
    rc.fillStyle = "rgba(12,14,18,0.72)";
    rc.fill();
    rc.clip();
    // Field of view (half the camera's horizontal angle each side).
    const half = Math.atan(Math.tan((camera.fov * Math.PI) / 180 / 2) * camera.aspect);
    rc.beginPath();
    rc.moveTo(c, c);
    rc.arc(c, c, c, -Math.PI / 2 - half, -Math.PI / 2 + half);
    rc.closePath();
    rc.fillStyle = "rgba(255,255,255,0.12)";
    rc.fill();
    if (me && origin) {
      const [px, pz] = local(me);
      const cos = Math.cos(view);
      const sin = Math.sin(view);
      // Local x east, z south -> screen, rotated so the view points up.
      const to = (x: number, z: number): [number, number] => {
        const dx = x - px;
        const dn = -(z - pz);
        return [c + (dx * cos - dn * sin) * k, c - (dx * sin + dn * cos) * k];
      };
      rc.lineCap = "round";
      for (const e of lastEdges) {
        rc.beginPath();
        for (let i = 0; i + 1 < e.line.length; i += 2) {
          const [sx, sy] = to(...local([e.line[i], e.line[i + 1]]));
          if (i === 0) rc.moveTo(sx, sy);
          else rc.lineTo(sx, sy);
        }
        rc.strokeStyle = STROKE[e.o] ?? STROKE[0];
        rc.lineWidth = e.o ? 5 : 3;
        rc.stroke();
      }
      // North on the rim.
      const nx = c + Math.sin(-view) * (c - 16);
      const ny = c - Math.cos(-view) * (c - 16);
      rc.fillStyle = "#fff";
      rc.font = "bold 22px system-ui";
      rc.textAlign = "center";
      rc.textBaseline = "middle";
      rc.fillText("N", nx, ny);
    }
    rc.beginPath();
    rc.arc(c, c, 7, 0, 2 * Math.PI);
    rc.fillStyle = "#4dabf7";
    rc.fill();
    rc.restore();
  }

  // "Align": turn the world so the nearest street lies along (or across) the view.
  alignBtn.onclick = () => {
    const me = ctx.me();
    if (!me || !origin) return;
    const best = nearestStreet(me);
    if (!best) return;
    const view = heading(camera.q);
    // Along the street, either way.
    const turn = [0, Math.PI].map((k) => wrap(best!.b + k - view)).reduce((a, b) => (Math.abs(a) < Math.abs(b) ? a : b));
    offset = wrap(offset - turn);
  };

  const draw = (now: number) => {
    const view = viewMatrix(camera.q, camera.pos);
    gl.viewport(0, 0, camera.vp[0], camera.vp[1]);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.disable(gl.CULL_FACE);
    gl.disable(gl.DEPTH_TEST);
    gl.enable(gl.BLEND);
    // Premultiplied "over", not additive: must stay visible on a bright camera image.
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
    const proj = new Float32Array(camera.proj);
    const items: { z: number; draw: () => void }[] = [];
    if (streetCount)
      items.push({
        z: depth(view, streetCenter),
        draw: () => {
          gl.useProgram(street);
          gl.uniformMatrix4fv(su.u_proj, false, proj);
          gl.uniformMatrix4fv(su.u_view, false, new Float32Array(view));
          gl.uniform1f(su.u_time, (now / 1000) % 3600);
          gl.uniform3fv(su.u_cam, camera.pos);
          gl.bindVertexArray(sb.vao);
          gl.drawElements(gl.TRIANGLES, streetCount, gl.UNSIGNED_INT, 0);
        },
      });
    for (const b of beams) {
      const alpha = b.building ? 0.35 + 0.35 * (0.5 + 0.5 * Math.sin(now / 300)) : b.alpha;
      items.push({
        z: depth(view, [b.at[0], b.at[1] + BEAM_H / 2, b.at[2]]),
        draw: () => {
          gl.useProgram(beam);
          gl.uniformMatrix4fv(bu.u_proj, false, proj);
          gl.uniformMatrix4fv(bu.u_mv, false, new Float32Array(translated(view, b.at)));
          gl.uniformMatrix3fv(bu.u_nrm, false, mat3(view));
          gl.uniform3fv(bu.u_color, b.color);
          gl.uniform1f(bu.u_alpha, alpha);
          gl.bindVertexArray(bb.vao);
          gl.drawElements(gl.TRIANGLES, cyl.idx.length, gl.UNSIGNED_SHORT, 0);
        },
      });
    }
    // Back to front, as three sorted transparent objects.
    items.sort((a, b) => b.z - a.z);
    for (const it of items) it.draw();
    gl.bindVertexArray(null);
  };

  let raf = 0;
  const frame = (now: number) => {
    raf = requestAnimationFrame(frame);
    const w = host.clientWidth;
    const h = host.clientHeight;
    if (canvas.width !== Math.floor(w * pixelRatio) || canvas.height !== Math.floor(h * pixelRatio)) {
      canvas.width = Math.floor(w * pixelRatio);
      canvas.height = Math.floor(h * pixelRatio);
      camera.aspect = w / h;
      camera.fov = w < h ? 62 : 42;
      camera.proj = perspective(camera.fov, camera.aspect, 0.3, 400);
      camera.vp = [Math.round(w * pixelRatio), Math.round(h * pixelRatio)];
    }
    const dt = lastFrame ? Math.min(0.25, (now - lastFrame) / 1000) : 0;
    lastFrame = now;
    // Swipe offset turns the world about the vertical (positive = to the right).
    const target = cameraQuat(orient.alpha, orient.beta, orient.gamma, screenAngle(), offset);
    camera.q = dt ? qslerp(camera.q, target, 1 - Math.exp(-dt / TURN_S)) : target;
    const me = ctx.me();
    if (me && origin) {
      if (!eye || Math.hypot(...enu(me, eye)) > SNAP_M) eye = me;
      else {
        const k = 1 - Math.exp(-dt / GLIDE_S);
        eye = [eye[0] + (me[0] - eye[0]) * k, eye[1] + (me[1] - eye[1]) * k];
      }
      const [x, z] = local(eye);
      camera.pos = [x, EYE_M, z];
    }
    if (now - lastRadar > 100) {
      lastRadar = now;
      drawRadar(heading(camera.q));
    }
    draw(now);
  };
  raf = requestAnimationFrame(frame);

  return () => {
    cancelAnimationFrame(raf);
    clearInterval(rebuildTimer);
    removeEventListener("deviceorientationabsolute", onOrient as EventListener);
    removeEventListener("deviceorientation", onOrient as EventListener);
    for (const t of stream?.getTracks() ?? []) t.stop();
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    host.innerHTML = "";
    host.hidden = true;
  };
}

function screenAngle(): number {
  return screen.orientation?.angle ?? (window as Window & { orientation?: number }).orientation ?? 0;
}
