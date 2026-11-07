// Indoors (indoor.rs, indoor.ts): no claim before a scan, a sheet code by
// link places the player and a claim works, a screen junction refuses its
// fixed code but takes the screen's, old screen codes and codes far apart in
// no time are refused. Screenshot at 360x640.
//   node e2e/indoor.mjs [base, default http://127.0.0.1:5080] [out dir]
// Server: an indoor region (db/import/indoor-region.sh, e.g. the example
// graph) as ELEMENTS_REGION, ELEMENTS_OPS_TOKEN, ELEMENTS_INDOOR_KEY,
// ELEMENTS_POW_BITS=0; not a sim server (sim skips the checks).
// Env: E2E_TOKEN (ops token), E2E_REGION (region-40c3-test).
import { chromium } from "playwright";
import { tmpdir } from "node:os";
import { runnerImport } from "vite";

const [base = "http://127.0.0.1:5080", out = tmpdir()] = process.argv.slice(2);
const region = process.env.E2E_REGION ?? "region-40c3-test";
const { module: C } = await runnerImport(new URL("../src/indoor-code.ts", import.meta.url).pathname, { configFile: false, logLevel: "error" });
let failed = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
  if (!ok) failed++;
};

const res = await fetch(`${base}/api/ops/indoor?region=${region}`, { headers: { Authorization: `Bearer ${process.env.E2E_TOKEN}` } });
check(res.ok, "ops list", String(res.status));
const list = await res.json();
const sheets = list.nodes.filter((n) => !n.screen);
const screens = list.nodes.filter((n) => n.screen);
check(screens.length > 0 && screens.every((n) => n.secret && !n.code), "screens: secret, no fixed code");
const byLabel = (l) => list.nodes.find((n) => n.label === l);
const foyer = byLabel("Foyer") ?? sheets[0];
const far = byLabel("Lounge") ?? screens[screens.length - 1];
const screen = byLabel("OSM-Assembly") ?? screens[0];

const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const ctx = await browser.newContext({ viewport: { width: 360, height: 640 }, deviceScaleFactor: 2, locale: "de-DE" });
const page = await ctx.newPage();
page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
const got = [];
page.on("websocket", (ws) => ws.on("framereceived", (f) => {
  try {
    const m = JSON.parse(f.payload);
    if (m.t === "scan" || m.t === "claim" || m.t === "welcome") got.push(m);
  } catch {}
}));
const next = async (t, ms = 15_000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const i = got.findIndex((m) => m.t === t);
    if (i >= 0) return got.splice(i, 1)[0];
    await page.waitForTimeout(100);
  }
  return null;
};
const scan = async (code) => {
  await page.evaluate(() => 0);
  await page.goto(`${base}/?region=${region}&q=${code}`);
  return next("scan");
};

// First visit: watching; "Mitspielen" needs no position indoors.
await page.goto(`${base}/?region=${region}`);
let w = await next("welcome", 60_000);
check(w?.indoor === true && w.watch === true, "first visit watches, welcome says indoor");
if (await page.isVisible("#intro")) await page.click("#intro-ok");
w = await next("welcome", 30_000);
check(w?.indoor === true && !w.watch, "joined without a position");
if (await page.isVisible("#intro")) await page.click("#intro-ok");
await page.waitForTimeout(1500);
check(await page.isVisible("#scan"), "scan button shown");
check(await page.$eval("#claim", (b) => b.disabled), "no claim before a scan");

// A sheet by link.
let m = await scan(foyer.code);
check(m?.ok && m.node === foyer.id, "sheet code places the player", JSON.stringify(m));
if (await page.isVisible("#intro")) await page.click("#intro-ok");
await page.waitForFunction(() => document.getElementById("gps")?.textContent?.includes("Knoten"), null, { timeout: 10_000 }).catch(() => {});
check((await page.textContent("#gps"))?.includes("noch"), "hold countdown", await page.textContent("#gps"));
await page.waitForFunction(() => !document.getElementById("claim").disabled, null, { timeout: 15_000 }).catch(() => {});
await page.screenshot({ path: `${out}/indoor-held.png` });
await page.$$eval("#side-card, #first-card, .card-overlay", (els) => els.forEach((e) => (e.hidden = true))).catch(() => {});
// Own ground already (side by chance): the button points to the front, no claim.
const front = (await page.textContent("#claim"))?.includes("Front");
await page.click("#claim").catch(() => {});
const cl = front ? { node: foyer.id, ok: false, reason: "own" } : await next("claim");
// Earlier runs may have left a source there ("strong", "own"): only range counts here.
check(cl?.node === foyer.id && (cl.ok || !["range", "moving"].includes(cl.reason)), "claim at the scanned junction in range", JSON.stringify(cl ?? {}).slice(0, 120));

// Screen junction: fixed code refused, its screen code taken, old one refused.
const secret = C.unhex(screen.secret);
const wnow = C.windowOf(Date.now(), list.window_s);
m = await scan(`${screen.id.toString(36)}-${C.mac(secret, "s")}`);
check(m && !m.ok && m.reason === "screen", "fixed code at a screen refused", m?.reason);
m = await scan(C.screenCode(secret, screen.id, wnow - 5));
check(m && !m.ok && m.reason === "old", "old screen code refused", m?.reason);
// Foyer to the screen is ~40 m: on foot that takes a while.
await page.waitForTimeout(12_000);
m = await scan(C.screenCode(secret, screen.id, C.windowOf(Date.now(), list.window_s)));
check(m?.ok && m.node === screen.id, "screen code places the player", JSON.stringify(m));

// Another junction far off at once: locked like a GPS jump.
const farCode = far.code ?? C.screenCode(C.unhex(far.secret), far.id, wnow);
m = await scan(farCode);
check(m && !m.ok && m.reason === "moving", "far junction at once refused", JSON.stringify(m));
m = await scan("zz-aaaaaaaaaa");
check(m && !m.ok && m.reason === "unknown", "made-up code refused", m?.reason);

// A first visit by a code link joins at once and is placed.
const ctx2 = await browser.newContext({ viewport: { width: 360, height: 640 }, locale: "de-DE" });
const p2 = await ctx2.newPage();
const got2 = [];
p2.on("websocket", (ws) => ws.on("framereceived", (f) => {
  try {
    const x = JSON.parse(f.payload);
    if (x.t === "scan" || x.t === "welcome") got2.push(x);
  } catch {}
}));
await p2.goto(`${base}/?region=${region}&q=${foyer.code}`);
await p2.waitForTimeout(12_000);
check(got2.some((x) => x.t === "welcome" && !x.watch) && got2.some((x) => x.t === "scan" && x.ok), "new visitor by link: joined and placed", got2.map((x) => `${x.t}:${x.watch ?? x.ok}`).join(","));

await browser.close();
console.log(failed ? `${failed} failed` : "all ok");
process.exit(failed ? 1 : 0);
