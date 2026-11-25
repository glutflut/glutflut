// Test page for carrying you can feel (tragen.html, spill.ts). Not linked.
import { Feel, askMotion, feelView, type FeelState } from "./spill";
import { Sound } from "./sound";
import type { Faction } from "./net";

const s = new Sound();
s.setOn(true);
const $ = (id: string) => document.getElementById(id)!;
const val = (id: string) => Number(($(id) as HTMLInputElement).value);
let feel: Feel | null = null;
let draw: ((st: FeelState) => void) | null = null;
let gps: number | null = null;
let lastSound = 0;

function sim() {
  $("wf-v").textContent = String(val("wind-from"));
  $("ws-v").textContent = String(val("wind-speed"));
  $("wh-v").textContent = String(val("walk-heading"));
  $("wv-v").textContent = (val("walk-speed") / 10).toFixed(1);
  feel?.setWind({ fromDeg: val("wind-from"), speed: val("wind-speed") });
  if (gps === null) feel?.walk(val("walk-heading"), val("walk-speed") / 10);
}
for (const id of ["wind-from", "wind-speed", "walk-heading", "walk-speed"]) $(id).addEventListener("input", sim);

async function take(kind: Faction) {
  await askMotion();
  feel?.stop();
  feel = new Feel(kind, (lost) => {
    const now = Date.now();
    if (now - lastSound < 700) return;
    lastSound = now;
    if (kind === "water") s.slosh(lost * 20);
    else s.gust(lost * 400);
  });
  feel.start();
  const v = feelView(kind);
  $("view").replaceChildren(v.el);
  draw = v.draw;
  ($("fill").firstElementChild as HTMLElement).style.background = kind === "water" ? "#2f9bff" : "#ff7a1a";
  sim();
}
$("water").onclick = () => void take("water");
$("fire").onclick = () => void take("fire");
$("gps").onclick = () => {
  if (gps !== null) {
    navigator.geolocation.clearWatch(gps);
    gps = null;
    $("gps").textContent = "Mit GPS gehen";
    sim();
    return;
  }
  gps = navigator.geolocation.watchPosition((p) => feel?.move(p.coords.latitude, p.coords.longitude, p.timestamp), undefined, { enableHighAccuracy: true });
  $("gps").textContent = "GPS aus (Probe-Gehen)";
};

let t0 = performance.now();
function frame(t: number) {
  const dt = Math.min(0.1, (t - t0) / 1000);
  t0 = t;
  if (feel && draw) {
    const st = feel.tick(dt);
    draw(st);
    ($("fill").firstElementChild as HTMLElement).style.width = `${st.fill * 100}%`;
    $("num").textContent =
      `Füllung ${Math.round(st.fill * 100)} % · ` +
      (feel.kind === "water" ? `Neigung ${st.tilt.toFixed(0)}° · Bewegung ${st.shake.toFixed(1)} m/s²` : `Gegenwind ${st.headwind.toFixed(2)}`) +
      (st.sensors ? "" : " · keine Bewegungssensoren");
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
