#!/usr/bin/env node
// Simulated players for a test server (ELEMENTS_ALLOW_SIM=1): they stand around
// a place and send positions; they count as synthetic data, never as real.
//   node tools/bots.mjs [count=12] [lat,lon=47.66038,9.17639] [minutes=35] [base] [region]
const [n = "12", at = "47.66038,9.17639", minutes = "35", base = "https://dev.glutflut.com", region = "region-konstanz"] = process.argv.slice(2);
const [lat, lon] = at.split(",").map(Number);
const url = `${base.replace(/^http/, "ws")}/ws?region=${encodeURIComponent(region)}`;
const seen = new Set();
const bots = [];
for (let i = 0; i < +n; i++) {
  const ws = new WebSocket(url);
  ws.addEventListener("open", () => ws.send(JSON.stringify({ t: "hello", token: null, bot: true })));
  ws.addEventListener("message", (e) => {
    if (typeof e.data !== "string") return;
    const t = JSON.parse(e.data).t;
    if (!seen.has(t)) (seen.add(t), console.log("first message:", t));
  });
  ws.addEventListener("error", () => console.error(`bot ${i}: connection failed`));
  ws.addEventListener("close", (e) => console.error(`bot ${i}: closed ${e.code}`));
  bots.push(ws);
}
const step = () => {
  for (const ws of bots) {
    if (ws.readyState !== WebSocket.OPEN) continue;
    // Within about 50 m of the place.
    const a = Math.random() * 2 * Math.PI, r = Math.random() * 50;
    ws.send(JSON.stringify({ t: "pos", lat: lat + (r * Math.sin(a)) / 111_320, lon: lon + (r * Math.cos(a)) / (111_320 * Math.cos((lat * Math.PI) / 180)) }));
  }
};
const timer = setInterval(step, 5000);
setTimeout(step, 1500);
setTimeout(() => {
  clearInterval(timer);
  for (const ws of bots) ws.close();
  setTimeout(() => process.exit(0), 500);
}, +minutes * 60_000);
const open = () => bots.filter((b) => b.readyState === WebSocket.OPEN).length;
setInterval(() => console.log(new Date().toLocaleTimeString(), `${open()} of ${n} connected`), 60_000);
setTimeout(() => console.log(`${open()} of ${n} connected`), 4000);
