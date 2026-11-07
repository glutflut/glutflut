// A screen at an indoor junction (server indoor.rs): shows the junction's
// changing code as a QR. Everything comes from the address fragment (never
// sent to a server): `#r=<region>&n=<node>&k=<secret hex>&w=<window s>&b=<game
// base url>&l=<label>`; `&eink` redraws only when the code changes (no bar).
// Runs offline once loaded; the device clock must be right (±30 s).
import qrcode from "qrcode-generator";
import { codeLink, screenCode, unhex, windowOf } from "./indoor-code";

const f = new URLSearchParams(location.hash.slice(1));
const region = f.get("r") ?? "";
const node = Number(f.get("n"));
const key = f.get("k") ?? "";
const windowS = Number(f.get("w") ?? 30) || 30;
const base = f.get("b") ?? location.origin;
const eink = f.has("eink");
const $ = (id: string) => document.getElementById(id)!;

if (f.get("l")) $("label").textContent = f.get("l")!;
document.body.classList.toggle("eink", eink);

if (!region || !Number.isSafeInteger(node) || !/^[0-9a-f]{64}$/.test(key)) {
  $("err").hidden = false;
  $("err").textContent = "Link unvollständig (r, n, k). Die Links stehen auf der Aushang-Seite (indoor.html).";
} else {
  const secret = unhex(key);
  let shown = -1;
  const draw = () => {
    const w = windowOf(Date.now(), windowS);
    if (w !== shown) {
      shown = w;
      const q = qrcode(0, "M");
      q.addData(codeLink(base, region, screenCode(secret, node, w)));
      q.make();
      $("qr").innerHTML = q.createSvgTag({ cellSize: 4, margin: 4, scalable: true });
    }
    if (!eink) {
      const left = 1 - ((Date.now() / 1000) % windowS) / windowS;
      $("bar").style.transform = `scaleX(${left})`;
    }
  };
  draw();
  setInterval(draw, eink ? 1000 : 250);
  // Keep the screen on where the browser allows it.
  (navigator as unknown as { wakeLock?: { request(t: string): Promise<unknown> } }).wakeLock?.request("screen").catch(() => {});
}
