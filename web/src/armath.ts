// Camera maths for the AR view, as three.js did it (checked against three in
// scripts/armath-check.mjs). World: x east, y up, -z north. Quaternions are
// [x, y, z, w]; matrices are column-major (as WebGL wants them).

export type Quat = [number, number, number, number];
export type Vec3 = [number, number, number];
export type Mat4 = Float64Array;

const RAD = Math.PI / 180;

export function qmul(a: Quat, b: Quat): Quat {
  const [ax, ay, az, aw] = a;
  const [bx, by, bz, bw] = b;
  return [
    ax * bw + aw * bx + ay * bz - az * by,
    ay * bw + aw * by + az * bx - ax * bz,
    az * bw + aw * bz + ax * by - ay * bx,
    aw * bw - ax * bx - ay * by - az * bz,
  ];
}

/** Spherical interpolation from `a` to `b` by `t` (0..1), the shorter way. */
export function qslerp(a: Quat, b: Quat, t: number): Quat {
  let [bx, by, bz, bw] = b;
  let cos = a[0] * bx + a[1] * by + a[2] * bz + a[3] * bw;
  if (cos < 0) {
    [bx, by, bz, bw] = [-bx, -by, -bz, -bw];
    cos = -cos;
  }
  let ka = 1 - t;
  let kb = t;
  if (cos < 0.9995) {
    const th = Math.acos(cos);
    const s = Math.sin(th);
    ka = Math.sin((1 - t) * th) / s;
    kb = Math.sin(t * th) / s;
  }
  const q: Quat = [ka * a[0] + kb * bx, ka * a[1] + kb * by, ka * a[2] + kb * bz, ka * a[3] + kb * bw];
  const n = Math.hypot(...q);
  return [q[0] / n, q[1] / n, q[2] / n, q[3] / n];
}

/** Rotation by `angle` (rad) about a unit axis. */
export function qaxis(x: number, y: number, z: number, angle: number): Quat {
  const s = Math.sin(angle / 2);
  return [x * s, y * s, z * s, Math.cos(angle / 2)];
}

export function qrotate([qx, qy, qz, qw]: Quat, [vx, vy, vz]: Vec3): Vec3 {
  const tx = 2 * (qy * vz - qz * vy);
  const ty = 2 * (qz * vx - qx * vz);
  const tz = 2 * (qx * vy - qy * vx);
  return [vx + qw * tx + qy * tz - qz * ty, vy + qw * ty + qz * tx - qx * tz, vz + qw * tz + qx * ty - qy * tx];
}

/** Device orientation (degrees) and screen angle -> camera rotation, as
 *  three's former DeviceOrientationControls: Euler YXZ (beta, alpha, -gamma),
 *  then -90° about x (camera looks out of the back), then the screen turn. */
export function deviceQuat(alpha: number, beta: number, gamma: number, screen: number): Quat {
  const euler = qmul(qmul(qaxis(0, 1, 0, alpha * RAD), qaxis(1, 0, 0, beta * RAD)), qaxis(0, 0, 1, -gamma * RAD));
  return qmul(qmul(euler, [-Math.SQRT1_2, 0, 0, Math.SQRT1_2]), qaxis(0, 0, 1, -screen * RAD));
}

/** With the swipe offset (rad): turns the world about the vertical. */
export function cameraQuat(alpha: number, beta: number, gamma: number, screen: number, offset: number): Quat {
  return qmul(qaxis(0, 1, 0, -offset), deviceQuat(alpha, beta, gamma, screen));
}

/** Heading of the view (rad, 0 north, clockwise). */
export function heading(q: Quat): number {
  const [x, , z] = qrotate(q, [0, 0, -1]);
  return Math.atan2(x, -z);
}

/** World -> camera for a camera at `p` with rotation `q`. */
export function viewMatrix([x, y, z, w]: Quat, [px, py, pz]: Vec3): Mat4 {
  // Rotation matrix columns of q; the view is its transpose.
  const r00 = 1 - 2 * (y * y + z * z), r01 = 2 * (x * y - w * z), r02 = 2 * (x * z + w * y);
  const r10 = 2 * (x * y + w * z), r11 = 1 - 2 * (x * x + z * z), r12 = 2 * (y * z - w * x);
  const r20 = 2 * (x * z - w * y), r21 = 2 * (y * z + w * x), r22 = 1 - 2 * (x * x + y * y);
  // prettier-ignore
  return new Float64Array([
    r00, r01, r02, 0,
    r10, r11, r12, 0,
    r20, r21, r22, 0,
    -(r00 * px + r10 * py + r20 * pz), -(r01 * px + r11 * py + r21 * pz), -(r02 * px + r12 * py + r22 * pz), 1,
  ]);
}

/** OpenGL perspective, vertical field of view in degrees. */
export function perspective(fovDeg: number, aspect: number, near: number, far: number): Mat4 {
  const top = near * Math.tan((fovDeg * RAD) / 2);
  const m = new Float64Array(16);
  m[0] = near / (top * aspect);
  m[5] = near / top;
  m[10] = -(far + near) / (far - near);
  m[11] = -1;
  m[14] = (-2 * far * near) / (far - near);
  return m;
}

/** `m` · translate(t). */
export function translated(m: Mat4, [x, y, z]: Vec3): Mat4 {
  const out = new Float64Array(m);
  for (let r = 0; r < 4; r++) out[12 + r] = m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r];
  return out;
}

/** Depth in front of the camera (m) of a world point, for back-to-front sorting. */
export function depth(view: Mat4, [x, y, z]: Vec3): number {
  return -(view[2] * x + view[6] * y + view[10] * z + view[14]);
}

/** Upper-left 3×3 (the rotation of a view matrix, its own normal matrix). */
export function mat3(m: Mat4): Float32Array {
  return new Float32Array([m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]]);
}

/** sRGB hex -> linear RGB, as three's colour management did for uniforms. */
export function linearRgb(hex: number): Vec3 {
  const lin = (c: number) => (c < 0.04045 ? c * 0.0773993808 : Math.pow(c * 0.9478672986 + 0.0521327014, 2.4));
  return [lin(((hex >> 16) & 255) / 255), lin(((hex >> 8) & 255) / 255), lin((hex & 255) / 255)];
}

/** Open cylinder standing on y=0, as three's CylinderGeometry(r, r, h, n, 1,
 *  true) moved up by h/2: interleaved position 3, normal 3, uv 2. */
export function cylinder(r: number, h: number, n: number): { v: Float32Array; idx: Uint16Array } {
  const v: number[] = [];
  for (let row = 0; row <= 1; row++) {
    for (let i = 0; i <= n; i++) {
      const a = (i / n) * 2 * Math.PI;
      const s = Math.sin(a);
      const c = Math.cos(a);
      v.push(r * s, (1 - row) * h, r * c, s, 0, c, i / n, 1 - row);
    }
  }
  const idx: number[] = [];
  for (let i = 0; i < n; i++) {
    const a = i, b = n + 1 + i, c = n + 2 + i, d = i + 1;
    idx.push(a, b, d, b, c, d);
  }
  return { v: new Float32Array(v), idx: new Uint16Array(idx) };
}
