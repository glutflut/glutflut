// Links into another region (server: remote.rs, handlers/far.rs) on raw
// sockets, against one server with two regions of the same cell:
//   1. own far link: P places B in Y and A in X; at A, peek and claim through B;
//   2. gift: P gives a key to B; Q places A2 in X and claims through the key;
//      a second gift of B the same week is refused.
// Needs ELEMENTS_ALLOW_SIM=1 and a short cooldown, e.g.
//   ELEMENTS_RULES_OVERRIDE='claim_cooldown_s=1'
// Usage: node e2e/far.mjs <x-slug> <y-slug> [base, default http://127.0.0.1:5080]
// FAR_Z=<slug>: also a third, not yet loaded region of the same cell (sleeping).
// FAR_EVICT=<slug> (server: ELEMENTS_MAX_REGIONS=2): Y sleeps with its sources, a claim wakes it.
// FAR_CITY=<key>: also claim through that city key into Y.
// FAR_WATCH=<s>: after the own far claim, print its build-up in Y every 10 s (fresh connections).
import { answerPow } from "./pow.mjs";

const [X, Y, BASE = "http://127.0.0.1:5080"] = process.argv.slice(2);
if (!X || !Y) throw new Error("usage: far.mjs <x-slug> <y-slug> [base]");
const WS = BASE.replace(/^http/, "ws");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
  if (!ok) failed++;
};

/** The region's junctions (the server loads it on first ask). */
async function nodes(slug) {
  for (let i = 0; i < 60; i++) {
    const res = await fetch(`${BASE}/api/region?region=${slug}`).catch(() => null);
    if (res?.ok) {
      const j = await res.json();
      return j.nodes.features.map((f) => ({ id: f.id, lon: f.geometry.coordinates[0], lat: f.geometry.coordinates[1] }));
    }
    await sleep(1000);
  }
  throw new Error(`region ${slug} did not load`);
}

/** Does the player with `token` see the source at `node` in Y as theirs? */
async function ownIn(token, node) {
  const w = await new Player("own@Y", Y, token).open();
  await sleep(2000);
  const r = w.msgs.flatMap((m) => (m.t === "sources" ? m.items : m.t === "sourcesdiff" ? m.set : [])).filter((r) => r[0] === node).at(-1);
  w.close();
  return r?.[4] === true;
}

class Player {
  constructor(name, region, token) {
    Object.assign(this, { name, region, token, msgs: [], waiters: [] });
  }
  async open() {
    this.ws = new WebSocket(`${WS}/ws?region=${this.region}`);
    this.ws.addEventListener("message", (e) => {
      const m = JSON.parse(e.data);
      this.msgs.push(m);
      this.waiters = this.waiters.filter((w) => !w(m));
    });
    await new Promise((r) => this.ws.addEventListener("open", r, { once: true }));
    const hello = { t: "hello", token: this.token ?? null };
    answerPow(this.ws, hello);
    this.send(hello);
    const w = await this.next("welcome", () => true, 20_000);
    this.token = w.token;
    this.send({ t: "side", faction: process.env.FAR_SIDE ?? "water" });
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
  /** Claim and keep at one of `cands` (first that works): its node and ticket. */
  async place(cands) {
    for (const n of cands) {
      await this.at(n);
      this.send({ t: "claim", node: n.id });
      const c = await this.next("claim");
      if (process.env.FAR_DEBUG) console.log(this.name, "claim", n.id, c.ok, c.reason ?? "");
      if (!c.ok) continue;
      await sleep(1200);
      this.send({ t: "keep", node: n.id });
      const k = await this.next("keep");
      if (process.env.FAR_DEBUG) console.log(this.name, "keep", n.id, k.ok, k.reason ?? "", k.wait);
      if (k.ok && k.ticket) return { node: n, ticket: k.ticket };
      await sleep(1200);
    }
    throw new Error(`${this.name}: no permanent source placed`);
  }
  close() {
    this.ws.close();
  }
}

// Spread candidates: every 40th junction (keeps want a gap), from a random
// start, so a rerun doesn't land where an earlier run already took everything.
const shift = Math.floor(Math.random() * 1000);
// Only spots with enough junctions within 80 m (else "nothing to take" is right).
const distM = (a, b) => {
  const k = Math.cos((a.lat * Math.PI) / 180) * 111_320;
  return Math.hypot((a.lon - b.lon) * k, (a.lat - b.lat) * 110_540);
};
const dense = (ns, n) => ns.filter((m) => Math.abs(m.lat - n.lat) < 0.001 && distM(m, n) <= 80).length >= 8;
const spread = (ns, from) =>
  ns
    .filter((_, i) => i % 40 === from % 40)
    .slice(shift % 200)
    .filter((n) => dense(ns, n))
    .slice(0, 25);

const [nx, ny] = [await nodes(X), await nodes(Y)];
console.log(`${X}: ${nx.length} junctions, ${Y}: ${ny.length}`);

// 1. Own far link.
const py = await new Player("P@Y", Y).open();
const b = await py.place(spread(ny, 7));
check(true, `B placed in ${Y}`, `node ${b.node.id}`);
py.send({ t: "gift", node: b.node.id, ticket: b.ticket });
const gift = await py.next("gift");
check(gift.ok && /^[0-9a-f]{32}$/.test(gift.key ?? ""), "gift key made");
py.send({ t: "gift", node: b.node.id, ticket: b.ticket });
const again = await py.next("gift");
check(!again.ok && again.reason === "soon", "second gift same week refused", again.reason);
py.close();

const px = await new Player("P@X", X, py.token).open();
const a = await px.place(spread(nx, 11));
check(true, `A placed in ${X}`, `node ${a.node.id}`);
await sleep(1200);
const far = { region: Y, key: b.ticket };
px.send({ t: "peek", via: a.ticket, far });
const pk = await px.next("peek");
// "linked": the far end answered, there's just nothing to take (test regions fill up).
check((pk.node !== undefined && pk.lon !== undefined) || pk.reason === "linked", "peek through own far link", pk.node ? `node ${pk.node}` : pk.reason);
px.send({ t: "claim", node: 0, via: a.ticket, far });
const cl = await px.next("claim");
check((cl.ok && Array.isArray(cl.far)) || cl.reason === "linked", "claim through own far link", cl.ok ? `+${cl.points} at ${cl.far}` : cl.reason);
check(!cl.ok || (Array.isArray(cl.near) && cl.near.length > 0 && Array.isArray(cl.from)), "answer carries the place and its source", cl.ok ? `${cl.near?.length} junctions, ${cl.streets?.length} streets` : "");
if (cl.ok) check(await ownIn(px.token, cl.node), "the far claim is the player's own in Y (white ring)");
// FAR_WATCH: the server's build-up of the new source in Y.
if (cl.ok && process.env.FAR_WATCH) {
  const t0 = Date.now();
  while (Date.now() - t0 < Number(process.env.FAR_WATCH) * 1000) {
    // A fresh connection gets the full list: the server's build-up now.
    const w = await new Player("W@Y", Y).open();
    await w.at({ lon: cl.far[0], lat: cl.far[1] });
    await sleep(2000);
    const r = w.msgs.flatMap((m) => (m.t === "sources" ? m.items : m.t === "sourcesdiff" ? m.set : [])).filter((r) => r[0] === cl.node).at(-1);
    console.log(`  +${((Date.now() - t0) / 1000) | 0}s [left ${r?.[2]}, life ${r?.[3]}, build ${r?.[7]}, ramp ${r?.[8]}, kind "${r?.[9]}"]`);
    w.close();
    await sleep(8000);
  }
}
// Away from A: refused, not an ordinary claim.
await px.at(nx[((nx.indexOf(a.node) + nx.length / 2) | 0) % nx.length]);
await sleep(1200);
px.send({ t: "claim", node: 0, via: a.ticket, far });
const away = await px.next("claim");
check(!away.ok && away.reason === "linkfar", "away from A: linkfar", away.reason);
// A wrong key.
await px.at(a.node);
px.send({ t: "claim", node: 0, via: a.ticket, far: { region: Y, key: "0".repeat(32) } });
const bad = await px.next("claim");
check(!bad.ok && bad.reason === "nokey", "wrong key: nokey", bad.reason);
px.send({ t: "peek", via: a.ticket, far: { region: Y, key: "0".repeat(32) } });
const pbad = await px.next("peek");
check(pbad.node === undefined && pbad.reason === "nokey", "peek with a wrong key says nokey", pbad.reason);
// A sleeping region (FAR_Z, not loaded yet): a peek doesn't wake it, a claim does.
if (process.env.FAR_Z) {
  const z = { region: process.env.FAR_Z, key: "0".repeat(32) };
  await sleep(1200);
  px.send({ t: "peek", via: a.ticket, far: z });
  const pz = await px.next("peek");
  check(pz.reason === "asleep", "peek into a sleeping region: asleep, not woken", pz.reason);
  const t0 = Date.now();
  px.send({ t: "claim", node: 0, via: a.ticket, far: z });
  const waitClaim = px.next("claim", () => true, 100_000);
  // While it loads, a look still says asleep (no 3 s timeout).
  await sleep(1500);
  px.send({ t: "peek", via: a.ticket, far: z });
  const pl = await px.next("peek");
  check(pl.reason === "asleep", "peek while it loads: asleep", pl.reason);
  const cz = await waitClaim;
  check(cz.reason === "nokey", "claim wakes it and gets its answer", `${cz.reason} after ${((Date.now() - t0) / 1000).toFixed(1)} s`);
}
px.close();

// 2. Gift: another player with their own A2 in X.
const qx = await new Player("Q@X", X).open();
const a2 = await qx.place(spread(nx, 23));
await sleep(1200);
const gf = { region: Y, key: gift.key };
qx.send({ t: "claim", node: 0, via: a2.ticket, far: gf });
const g1 = await qx.next("claim");
// "linked": the key was accepted, there's just nothing to take around B now.
const through = (m) => m.ok || m.reason === "linked";
check(through(g1), "claim through a gift key", g1.ok ? `+${g1.points}` : g1.reason);
// Q was never in Y: the claim becomes theirs once they come.
if (g1.ok) check(await ownIn(qx.token, g1.node), "a newcomer's far claim is theirs on arrival in Y");
// FAR_CITY=<key>: a city key to a junction of Y (editor_keys.rs), either side.
if (process.env.FAR_CITY) {
  await sleep(1200);
  qx.send({ t: "claim", node: 0, via: a2.ticket, far: { region: Y, key: process.env.FAR_CITY } });
  const ck = await qx.next("claim");
  check(through(ck), "claim through a city key", ck.ok ? `+${ck.points} from ${ck.from}` : ck.reason);
}
// Y asleep with its permanent sources (FAR_EVICT: a third region; server with
// ELEMENTS_MAX_REGIONS=2, so loading it puts Y to sleep): a claim wakes Y
// and must find the source (not `nokey` before Y has it back on the map).
if (process.env.FAR_EVICT) {
  await nodes(process.env.FAR_EVICT);
  let slept = false;
  for (let i = 0; i < 60 && !slept; i++) {
    await sleep(1500);
    qx.send({ t: "peek", via: a2.ticket, far: gf });
    slept = (await qx.next("peek")).reason === "asleep";
  }
  check(slept, `${Y} asleep after loading ${process.env.FAR_EVICT}`);
  const t0 = Date.now();
  qx.send({ t: "claim", node: 0, via: a2.ticket, far: gf });
  const w = await qx.next("claim", () => true, 100_000);
  check(through(w), "claim wakes Y and goes through its source (not nokey)", `${w.ok ? `+${w.points}` : w.reason} after ${((Date.now() - t0) / 1000).toFixed(1)} s`);
}
qx.close();

console.log(failed ? `${failed} failed` : "all ok");
process.exit(failed ? 1 : 0);
