// Layout check of the small map after a claim into another region
// (inset.ts): on a small phone it must stay clear of the claim buttons and
// the top bar. Needs `?sim` (window.__inset).
//   node e2e/inset.mjs "<app url with ?sim&at=…>" <region> <lon,lat> out.png [server]
import { chromium } from "playwright";

const [url, region, at, out = "inset.png", base = "http://127.0.0.1:5080"] = process.argv.slice(2);
const [lon, lat] = at.split(",").map(Number);
// Real junctions around the spot, as the server sends them (lon, lat, fire,
// water): fire to the west, water to the east, the spot claimed by fire;
// streets to the two nearest junctions each (the server sends real ones).
const j = await (await fetch(`${base}/api/region?region=${region}`)).json();
const m = (p) => Math.hypot((p[0] - lon) * 111_320 * Math.cos((lat * Math.PI) / 180), (p[1] - lat) * 110_540);
const near = j.nodes.features
  .map((f) => f.geometry.coordinates)
  .filter((p) => m(p) <= 200 && m(p) > 0.5)
  .map((p) => [p[0], p[1], p[0] < lon - 0.0004 ? 0.8 : 0, p[0] > lon + 0.0004 ? 0.6 : 0]);
near.unshift([lon, lat, 1, 0]);
const streets = [];
near.forEach((p, a) => {
  near
    .map((q, b) => [b, Math.hypot(q[0] - p[0], q[1] - p[1])])
    .filter(([b]) => b !== a)
    .sort((x, y) => x[1] - y[1])
    .slice(0, 2)
    .forEach(([b]) => streets.push([a, b]));
});
console.log(`${near.length} junctions, ${streets.length} streets`);

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const page = await browser.newPage({ viewport: { width: 360, height: 640 }, deviceScaleFactor: 2 });
page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
await page.goto(url);
await page.waitForFunction(() => typeof window.__inset === "function", null, { timeout: 30_000 });
await page.waitForTimeout(3000);
await page.evaluate(([r, ll, n, st, fr]) => window.__inset(r, "Calw", ll, n, st, fr), [region, [lon, lat], near, streets, near[Math.min(9, near.length - 1)].slice(0, 2)]);
await page.waitForTimeout(1500);
const box = (sel) => page.$eval(sel, (e) => {
  const b = e.getBoundingClientRect();
  return { top: Math.round(b.top), bottom: Math.round(b.bottom), left: Math.round(b.left), right: Math.round(b.right), hidden: e.hidden || b.height === 0 };
});
const inset = await box("#inset");
const hud = await box("#hud");
const claim = await box("#claim");
console.log("inset", JSON.stringify(inset));
console.log("hud", JSON.stringify(hud));
console.log("claim", JSON.stringify(claim));
const clear = inset.top >= hud.bottom && inset.bottom <= claim.top && !inset.hidden;
console.log(clear ? "ok   inset clear of top bar and claim buttons" : "FAIL inset overlaps");
await page.screenshot({ path: out });
await page.waitForTimeout(8500);
console.log((await box("#inset")).hidden ? "ok   faded out" : "FAIL still shown after 10 s");
await browser.close();
process.exit(clear ? 0 : 1);
