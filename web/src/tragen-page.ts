// Test page for carrying you can feel (tragen.html, spill.ts). Not linked.
import { Feel, askMotion, feelView, type FeelState } from "./spill";
import { Sound } from "./sound";
import type { Faction } from "./net";

const s = new Sound();
s.setOn(true);
const $ = (id: string) => document.getElementById(id)!;
let feel: Feel | null = null;
let draw: ((st: FeelState) => void) | null = null;
let lastSound = 0;

async function take(kind: Faction) {
  await askMotion();
  feel?.stop();
  feel = new Feel(kind, (lost) => {
    const now = Date.now();
    if (now - lastSound < 700) return;
    lastSound = now;
    if (kind === "water") s.slosh(lost * 20);
    else s.tumble(lost * 20);
  });
  feel.start();
  const v = feelView(kind);
  $("view").replaceChildren(v.el);
  draw = v.draw;
  ($("fill").firstElementChild as HTMLElement).style.background = kind === "water" ? "#2f9bff" : "#ff7a1a";
}
$("water").onclick = () => void take("water");
$("fire").onclick = () => void take("fire");

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
      `Neigung ${st.tilt.toFixed(0)}° · Bewegung ${st.shake.toFixed(1)} m/s²` +
      (st.sensors ? "" : " · keine Bewegungssensoren");
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
