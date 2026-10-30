// Layout check of the small map after a claim into another region
// (inset.ts): on a small phone it must stay clear of the claim buttons and
// the top bar. Needs `?sim` (window.__inset).
//   node e2e/inset.mjs "<app url with ?sim&at=…>" <region> <lon,lat> out.png
import { chromium } from "playwright";

const [url, region, at, out = "inset.png"] = process.argv.slice(2);
const [lon, lat] = at.split(",").map(Number);
// Junctions around the spot as the server sends them: a ring of both sides.
const near = [[lon, lat, 1]];
for (let i = 0; i < 40; i++) {
  const a = (i / 40) * 2 * Math.PI;
  const r = 0.0004 + 0.0012 * ((i * 7) % 10) / 10;
  near.push([lon + r * Math.cos(a) * 1.5, lat + r * Math.sin(a), i % 3]);
}

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const page = await browser.newPage({ viewport: { width: 360, height: 640 }, deviceScaleFactor: 2 });
page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
await page.goto(url);
await page.waitForFunction(() => typeof window.__inset === "function", null, { timeout: 30_000 });
await page.waitForTimeout(3000);
await page.evaluate(([r, ll, n]) => window.__inset(r, "Calw", ll, n), [region, [lon, lat], near]);
await page.waitForTimeout(4000);
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
await page.waitForTimeout(5000);
console.log((await box("#inset")).hidden ? "ok   faded out" : "FAIL still shown after 12 s");
await browser.close();
process.exit(clear ? 0 : 1);
