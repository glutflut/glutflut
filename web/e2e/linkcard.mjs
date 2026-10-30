// Junction card of an own source linked to another city (perm.ts
// linkControl): "↔ <city>" and "unlink", clear of the buttons at 360x640.
// Server with ELEMENTS_ALLOW_SIM=1, rules/default.toml, POW 0; vite on 5173.
//   node e2e/linkcard.mjs out.png
import { chromium } from "playwright";
import { answerPow } from "./pow.mjs";
const [out] = process.argv.slice(2);
const BASE = "http://127.0.0.1:5080", Y = "region-baden-baden";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let j;
for (let i = 0; i < 60 && !j; i++) { const r = await fetch(`${BASE}/api/region?region=${Y}`).catch(() => null); if (r?.ok) j = await r.json(); else await sleep(1000); }
const ns = j.nodes.features.map((f) => ({ id: f.id, lon: f.geometry.coordinates[0], lat: f.geometry.coordinates[1] }));
// A player places a permanent source on a raw socket.
const ws = new WebSocket(`${BASE.replace("http", "ws")}/ws?region=${Y}`);
const msgs = [];
const waiters = [];
ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); msgs.push(m); for (const w of [...waiters]) w(m); });
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
const send = (m) => ws.send(JSON.stringify(m));
const next = (t, ms = 8000) => new Promise((res, rej) => { const to = setTimeout(() => rej(new Error("no " + t)), ms); const w = (m) => { if (m.t === t) { clearTimeout(to); waiters.splice(waiters.indexOf(w), 1); res(m); } }; waiters.push(w); });
const hello = { t: "hello", token: null }; answerPow(ws, hello); send(hello);
const token = (await next("welcome", 20000)).token;
send({ t: "side", faction: "water" }); await next("side");
let placed = null;
for (const n of ns.filter((_, i) => i % 41 === 7).slice(Math.floor(Math.random() * 100))) {
  send({ t: "pos", lat: n.lat, lon: n.lon, acc: 5 }); await sleep(300);
  send({ t: "claim", node: n.id }); if (!(await next("claim")).ok) continue;
  await sleep(1200); send({ t: "keep", node: n.id }); const k = await next("keep");
  if (k.ok && k.ticket) { placed = { n, ticket: k.ticket }; break; }
}
ws.close();
console.log("placed", placed?.n.id);
const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
const page = await browser.newPage({ viewport: { width: 360, height: 640 }, deviceScaleFactor: 2 });
page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
await page.addInitScript(([tok, p, Y]) => {
  if (sessionStorage.getItem("seeded")) return; sessionStorage.setItem("seeded", "1");
  localStorage.setItem("elements.token", tok);
  localStorage.setItem(`elements.perm.${Y}`, JSON.stringify([{ ticket: p.ticket, node: p.n.id, ll: [p.n.lon, p.n.lat], far: { region: "region-goerlitz", key: "f".repeat(32), name: "Görlitz", ll: [14.9877, 51.1526] } }]));
}, [token, placed, Y]);
await page.goto(`http://127.0.0.1:5173/?sim&region=${Y}&at=${placed.n.lat},${placed.n.lon}`);
await page.waitForTimeout(6000);
for (let i = 0; i < 3; i++) await page.getByRole("button", { name: /Weiter|Los geht/ }).first().click({ timeout: 1500 }).catch(() => {});
await page.evaluate((id) => window.__card(id), placed.n.id);
await page.waitForTimeout(2500);
const link = await page.$eval("#node-link", (e) => (e.hidden ? "" : e.textContent ?? "")).catch(() => "");
const box = (sel) => page.$eval(sel, (e) => { const b = e.getBoundingClientRect(); return [Math.round(b.top), Math.round(b.bottom)]; }).catch(() => null);
const card = await box("#node-card"), claim = await box("#claim");
const ok = link.includes("↔ Görlitz") && card && claim && card[1] <= claim[0];
console.log(`${ok ? "ok  " : "FAIL"} card shows where the link goes (${link}; card ${card}, claim ${claim})`);
await page.screenshot({ path: out });
await browser.close();
process.exit(ok ? 0 : 1);
