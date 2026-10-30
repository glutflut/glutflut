// Found keys (server: handlers/far.rs `found`, found.rs) on raw sockets:
// a player with a permanent source of their side in Y claims the given
// junctions of X (hilltops for fire, junctions at a big river for water)
// until a Funkenflug / Flaschenpost key arrives, then claims through it.
// Server: ELEMENTS_ALLOW_SIM=1, a short cooldown and certain finds, e.g.
//   ELEMENTS_RULES_OVERRIDE='claim_cooldown_s=1; keep.ember_chance=1; keep.bottle_chance=1'
// FOUND_EVICT=<slug> (server ELEMENTS_MAX_REGIONS=2): Y sleeps first, so the
// key is written to the database and Y reads it on waking.
// Usage: node e2e/found.mjs <x-slug> <y-slug> <fire|water> <node,node,...> [base]
import { answerPow } from "./pow.mjs";

const [X, Y, SIDE, NODES, BASE = "http://127.0.0.1:5080"] = process.argv.slice(2);
if (!X || !Y || !SIDE || !NODES) throw new Error("usage: found.mjs <x> <y> <fire|water> <nodes> [base]");
const WS = BASE.replace(/^http/, "ws");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
  if (!ok) failed++;
};

async function nodes(slug) {
  for (let i = 0; i < 90; i++) {
    const res = await fetch(`${BASE}/api/region?region=${slug}`).catch(() => null);
    if (res?.ok) {
      const j = await res.json();
      return j.nodes.features.map((f) => ({ id: f.id, lon: f.geometry.coordinates[0], lat: f.geometry.coordinates[1] }));
    }
    await sleep(1000);
  }
  throw new Error(`region ${slug} did not load`);
}

class Player {
  constructor(name, region, token) {
    Object.assign(this, { name, region, token, waiters: [] });
  }
  async open() {
    this.ws = new WebSocket(`${WS}/ws?region=${this.region}`);
    this.ws.addEventListener("message", (e) => {
      const m = JSON.parse(e.data);
      this.waiters = this.waiters.filter((w) => !w(m));
    });
    await new Promise((r) => this.ws.addEventListener("open", r, { once: true }));
    const hello = { t: "hello", token: this.token ?? null };
    answerPow(this.ws, hello);
    this.send(hello);
    this.token = (await this.next("welcome", () => true, 20_000)).token;
    this.send({ t: "side", faction: SIDE });
    await this.next("side");
    return this;
  }
  send(m) {
    this.ws.send(JSON.stringify(m));
  }
  next(t, ok = () => true, ms = 8000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.name}: no ${t}`)), ms);
      this.waiters.push((m) => {
        if (m.t !== t || !ok(m)) return false;
        clearTimeout(timer);
        resolve(m);
        return true;
      });
    });
  }
  async at(n) {
    this.send({ t: "pos", lat: n.lat, lon: n.lon, acc: 5 });
    await sleep(300);
  }
  async place(cands) {
    for (const n of cands) {
      await this.at(n);
      this.send({ t: "claim", node: n.id });
      if (!(await this.next("claim")).ok) continue;
      await sleep(1200);
      this.send({ t: "keep", node: n.id });
      const k = await this.next("keep");
      if (k.ok && k.ticket) return { node: n, ticket: k.ticket };
      await sleep(1200);
    }
    throw new Error(`${this.name}: no permanent source placed`);
  }
  close() {
    this.ws.close();
  }
}

const shift = Math.floor(Math.random() * 200);
const spread = (ns) => ns.filter((_, i) => i % 37 === 5).slice(shift, shift + 25);

const ny = await nodes(Y);
const py = await new Player("P@Y", Y).open();
const b = await py.place(spread(ny));
check(true, `${SIDE} source placed in ${Y}`, `node ${b.node.id}`);
py.close();

if (process.env.FOUND_EVICT) {
  await nodes(process.env.FOUND_EVICT);
  await sleep(4000);
  console.log(`(loaded ${process.env.FOUND_EVICT} so that ${Y} sleeps)`);
}

const nx = await nodes(X);
const byId = new Map(nx.map((n) => [n.id, n]));
const spots = NODES.split(",").map((s) => byId.get(Number(s))).filter(Boolean);
const px = await new Player("P@X", X, py.token).open();
let found = null;
const gotFound = px.next("found", () => true, 600_000).then((m) => (found = m));
for (const n of spots) {
  if (found) break;
  await px.at(n);
  px.send({ t: "claim", node: n.id });
  await px.next("claim").catch(() => null);
  await sleep(1500);
}
await Promise.race([gotFound, sleep(20_000)]);
check(!!found && found.region === Y, `found key to ${Y}`, found ? `${found.origin}, ${found.name}` : `none after ${spots.length} claims`);

if (found) {
  const a = await px.place(spread(nx));
  await sleep(1200);
  const far = { region: Y, key: found.key };
  px.send({ t: "claim", node: 0, via: a.ticket, far });
  const c = await px.next("claim", () => true, 100_000);
  check(c.ok || c.reason === "linked", "claim through the found key", c.ok ? `+${c.points}` : c.reason);
}
px.close();
console.log(failed ? `${failed} failed` : "all ok");
process.exit(failed ? 1 : 0);
