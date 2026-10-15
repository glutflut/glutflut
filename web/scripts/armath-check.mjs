// AR camera maths (src/armath.ts) against three.js, over a grid of device
// orientations, screen angles, swipe offsets and positions.
//   node scripts/armath-check.mjs   (from web/; needs three, a dev dependency)
import * as THREE from "three";
import { runnerImport } from "vite";

const { module: M } = await runnerImport(new URL("../src/armath.ts", import.meta.url).pathname, { configFile: false, logLevel: "error" });

let worst = 0;
let n = 0;
const check = (what, a, b, tol = 1e-12) => {
  for (let i = 0; i < a.length; i++) {
    const d = Math.abs(a[i] - b[i]);
    worst = Math.max(worst, d);
    n++;
    if (!(d <= tol)) throw new Error(`${what}[${i}]: ${a[i]} vs ${b[i]}`);
  }
};

// Old ar.ts, verbatim.
const zee = new THREE.Vector3(0, 0, 1);
const euler = new THREE.Euler();
const q0 = new THREE.Quaternion();
const q1 = new THREE.Quaternion(-Math.sqrt(0.5), 0, 0, Math.sqrt(0.5));
function orientationQuat(q, alpha, beta, gamma, screen) {
  const r = THREE.MathUtils.degToRad;
  euler.set(r(beta), r(alpha), -r(gamma), "YXZ");
  q.setFromEuler(euler).multiply(q1).multiply(q0.setFromAxisAngle(zee, -r(screen)));
}
function heading(q) {
  const f = new THREE.Vector3(0, 0, -1).applyQuaternion(q);
  return Math.atan2(f.x, -f.z);
}
const wrap = (a) => Math.atan2(Math.sin(a), Math.cos(a));

const camera = new THREE.PerspectiveCamera(62, 1, 0.3, 400);
const q = new THREE.Quaternion();
const yaw = new THREE.Quaternion();
const up = new THREE.Vector3(0, 1, 0);
const probe = new THREE.Vector4();
let cases = 0;
for (let alpha = 0; alpha < 360; alpha += 22.5)
  for (let beta = -180; beta <= 180; beta += 30)
    for (let gamma = -90; gamma <= 90; gamma += 22.5)
      for (const screen of [0, 90, 180, 270, -90])
        for (const offset of [0, 0.4, -2.1]) {
          cases++;
          orientationQuat(q, alpha, beta, gamma, screen);
          camera.quaternion.copy(yaw.setFromAxisAngle(up, -offset)).multiply(q);
          const pos = [alpha / 10 - 15, 1.5, gamma / 7];
          camera.position.set(...pos);
          camera.updateMatrixWorld();
          const ours = M.cameraQuat(alpha, beta, gamma, screen, offset);
          check("quat", ours, camera.quaternion.toArray());
          const dh = wrap(M.heading(ours) - heading(camera.quaternion));
          check("heading", [dh], [0], 1e-9);
          const view = M.viewMatrix(ours, pos);
          check("view", view, camera.matrixWorldInverse.elements, 1e-11);
          // Sorting key: three's clip z of a point is monotonic in our depth.
          const p = [3, 4.5, -7];
          probe.set(...p, 1).applyMatrix4(camera.matrixWorldInverse);
          check("depth", [M.depth(view, p)], [-probe.z], 1e-11);
        }

for (const [w, h] of [[390, 844], [844, 390], [1080, 2400], [800, 800]]) {
  camera.aspect = w / h;
  camera.fov = w < h ? 62 : 42;
  camera.updateProjectionMatrix();
  check("proj", M.perspective(camera.fov, camera.aspect, 0.3, 400), camera.projectionMatrix.elements, 1e-12);
}

const t = new THREE.Matrix4().makeTranslation(2, -0.4, 9);
const v = M.viewMatrix(M.cameraQuat(30, 70, 5, 90, 0.2), [1, 1.5, 2]);
check("translated", M.translated(v, [2, -0.4, 9]), new THREE.Matrix4().fromArray(v).multiply(t).elements, 1e-12);

// Smoothing: slerp as three's (shorter way, normalised).
for (const [a, b] of [[[10, 80, 3, 0], [40, 85, -5, 90]], [[0, 89, 0, 0], [350, 91, 2, 0]], [[120, 60, 30, 0], [121, 60, 30, 0]], [[0, 80, 0, 0], [180, 80, 0, 0]]])
  for (const t of [0, 0.1, 0.5, 0.9, 1]) {
    const qa = M.cameraQuat(...a, 0);
    const qb = M.cameraQuat(...b, 0);
    const three = new THREE.Quaternion(...qa).slerp(new THREE.Quaternion(...qb), t);
    const ours = M.qslerp(qa, qb, t);
    const s = Math.sign(ours[3] * three.w + ours[0] * three.x + ours[1] * three.y + ours[2] * three.z) || 1;
    check("slerp", ours.map((v) => v * s), three.toArray(), 1e-3);
  }

for (const hex of [0xff7a1a, 0x2f9bff]) check("colour", M.linearRgb(hex), new THREE.Color(hex).toArray(), 1e-7);

const g = new THREE.CylinderGeometry(0.35, 0.35, 9, 24, 1, true).translate(0, 4.5, 0);
const cyl = M.cylinder(0.35, 9, 24);
const pick = (k, len) => Array.from({ length: cyl.v.length / 8 }, (_, i) => Array.from(cyl.v.slice(i * 8 + k, i * 8 + k + len))).flat();
check("cyl.pos", pick(0, 3), g.attributes.position.array, 1e-6);
check("cyl.normal", pick(3, 3), g.attributes.normal.array, 1e-6);
check("cyl.uv", pick(6, 2), g.attributes.uv.array, 1e-6);
check("cyl.index", cyl.idx, g.index.array, 0);

console.log(`ok: ${cases} orientations, ${n} values, max abs diff ${worst.toExponential(2)}`);
