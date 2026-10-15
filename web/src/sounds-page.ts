// Listening page for the sounds (sounds.html): every event once, and the
// ambient layer with sliders. For tuning by ear; not linked from the app.
import { Sound } from "./sound";

const s = new Sound();
s.setOn(true);
s.setAmbient(true);
const events: [string, string, () => void][] = [
  ["Erobern", "fire", () => s.claim("fire")],
  ["Erobern", "water", () => s.claim("water")],
  ["Aufbau 6 s", "fire", () => s.building("fire", 6)],
  ["Aufbau 6 s", "water", () => s.building("water", 6)],
  ["Glut setzen", "fire", () => s.keep("fire")],
  ["Eis setzen", "water", () => s.keep("water")],
  ["Glut schüren", "fire", () => s.topup("fire")],
  ["Eis auffrischen", "water", () => s.topup("water")],
  ["Quelle verloren", "fire", () => s.lost("fire")],
  ["Quelle verloren", "water", () => s.lost("water")],
  ["Glutnest taucht auf", "fire", () => s.spark("fire")],
  ["Wasserader taucht auf", "water", () => s.spark("water")],
  ["Löschen (Dampf)", "", () => s.steam()],
  ["Fundstück", "", () => s.chime()],
  ["Tunnel", "", () => s.rumble()],
  ["Stufe / Abzeichen", "", () => s.motif()],
  ["Wieder bereit", "", () => s.tick()],
];
const grid = document.getElementById("events")!;
for (const [label, cls, play] of events) {
  const b = Object.assign(document.createElement("button"), { textContent: label, className: cls });
  b.onclick = play;
  grid.append(b);
}
const amb = () =>
  s.ground(
    Number((document.getElementById("amb-fire") as HTMLInputElement).value) / 100,
    Number((document.getElementById("amb-water") as HTMLInputElement).value) / 100,
  );
for (const id of ["amb-fire", "amb-water"]) document.getElementById(id)!.addEventListener("input", amb);
