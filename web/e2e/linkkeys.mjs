// Link keys ([keep] link_keys, handlers/keep.rs link, far.rs ask) on raw
// sockets, one server with two regions of the same cell:
//   local: keys come with placing; linking needs the player at the source
//   and a key of the other one, used once; unlinking is free;
//   far: a key of the far source binds to the first link that claims
//   through it; another near source, or the far source's ticket, gets nokey.
// Needs ELEMENTS_ALLOW_SIM=1, ELEMENTS_POW_BITS=0, rules/default.toml and a
// short cooldown (ELEMENTS_RULES_OVERRIDE='claim_cooldown_s=1').
// Usage: node e2e/linkkeys.mjs <x-slug> <y-slug> [base, default http://127.0.0.1:5080]
import { answerPow } from "./pow.mjs";

const [X, Y, BASE = "http://127.0.0.1:5080"] = process.argv.slice(2);
if (!X || !Y) throw new Error("usage: linkkeys.mjs <x-slug> <y-slug> [base]");
const WS = BASE.replace(/^http/, "ws");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
const check = (ok, what, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${detail ? ` (${detail})` : ""}`);
  if (!ok) failed++;
};
const distM = (a, b) => {
  const k = Math.cos((a.lat * Math.PI) / 180) * 111_320;
  return Math.hypot((a.lon - b.lon) * k, (a.lat - b.lat) * 110_540);
};

async function nodes(slug) {
  for (let i = 0; i < 60; i++) {
    const res = await fetch(`${BASE}/api/region?region=${slug}`).catch(() => null);
    if (res?.ok) return (await res.json()).nodes.features.map((f) => ({ id: f.id, lon: f.geometry.coordinates[0], lat: f.geometry.coordinates[1] }));
    await sleep(1000);
  }
  throw new Error(`region ${slug} did not load`);
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
    this.welcome = await this.next("welcome", () => true, 20_000);
    this.token = this.welcome.token;
    await sleep(300);
    this.send({ t: "side", faction: "water" });
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
      const cl = await this.next("claim");
      if (process.env.LK_DEBUG) console.log(this.name, "claim", n.id, cl.ok, cl.reason ?? "", cl.wait ?? "");
      if (!cl.ok) continue;
      await sleep(1200);
      this.send({ t: "keep", node: n.id });
      const k = await this.next("keep");
      if (process.env.LK_DEBUG) console.log(this.name, "keep", n.id, k.ok, k.reason ?? "");
      if (k.ok && k.ticket) return { node: n, ticket: k.ticket, keys: k.keys ?? [] };
      await sleep(1200);
    }
    throw new Error(`${this.name}: no permanent source placed`);
  }
  link(a, b, key) {
    this.send({ t: "link", node: a.node.id, ticket: a.ticket, to: b?.node.id ?? null, to_ticket: b?.ticket ?? null, key: key ?? null });
    return this.next("link");
  }
  close() {
    this.ws.close();
  }
}

const shift = Math.floor(Math.random() * 1000);
const dense = (ns, n) => ns.filter((m) => Math.abs(m.lat - n.lat) < 0.001 && distM(m, n) <= 80).length >= 8;
// Every 40th junction, rotated by a random start (reruns land elsewhere), dense spots only.
const spread = (ns, from) => {
  const c = ns.filter((_, i) => i % 40 === from % 40).filter((n) => dense(ns, n));
  const k = shift % Math.max(1, c.length);
  return [...c.slice(k), ...c.slice(0, k)].slice(0, 25);
};

const [nx, ny] = [await nodes(X), await nodes(Y)];
console.log(`${X}: ${nx.length} junctions, ${Y}: ${ny.length}`);

// --- local links in X ---
const p = await new Player("P@X", X).open();
check(p.welcome.keep_link_keys === 2, "welcome says link keys are on", `keep_link_keys ${p.welcome.keep_link_keys}`);
const a = await p.place(spread(nx, 11));
const b = await p.place(spread(nx, 23).filter((n) => distM(n, a.node) > 200));
check(a.keys.length >= 1 && a.keys.length <= 2 && b.keys.length >= 1, "keys come with placing", `A ${a.keys.length}, B ${b.keys.length}`);
// Standing at B (the last placed), link A to B: refused, A is not where we stand.
let r = await p.link(a, b, b.keys[0]);
check(!r.ok && r.reason === "range", "not at the source: range", r.reason);
// At A, with a key of B.
await p.at(a.node);
r = await p.link(a, b, "0".repeat(32));
check(!r.ok && r.reason === "nokey", "a made-up key: nokey", r.reason);
r = await p.link(a, b, a.keys[0]);
check(!r.ok && r.reason === "nokey", "a key of the wrong source: nokey", r.reason);
r = await p.link(a, b, b.keys[0]);
check(r.ok, "at A with a key of B: linked", r.reason ?? "");
r = await p.link(a, null);
check(r.ok, "unlinking is free");
r = await p.link(a, b, b.keys[0]);
check(!r.ok && r.reason === "nokey", "the same key again: used up", r.reason);
if (b.keys[1]) {
  r = await p.link(a, b, b.keys[1]);
  check(r.ok, "B's second key links again");
}
// A top-up gives no new key the same day (one a day).
await p.at(b.node);
await sleep(1200);
p.send({ t: "keep", node: b.node.id, ticket: b.ticket });
const tu = await p.next("keep");
check(tu.ok && tu.topup && !tu.keys, "top-up the same day: no new key", tu.reason ?? `keys ${tu.keys?.length ?? 0}`);

// --- far links X -> Y ---
const py = await new Player("P@Y", Y, p.token).open();
const c = await py.place(spread(ny, 7));
check(c.keys.length >= 1, "far source C has a key", `${c.keys.length}`);
py.close();
await p.at(a.node);
await sleep(1200);
p.send({ t: "claim", node: 0, via: a.ticket, far: { region: Y, key: c.ticket } });
r = await p.next("claim");
check(!r.ok && r.reason === "nokey", "far claim with C's ticket: nokey", r.reason);
await sleep(1200);
p.send({ t: "claim", node: 0, via: a.ticket, far: { region: Y, key: c.keys[0] } });
r = await p.next("claim");
check(r.ok || r.reason === "linked", "far claim from A with C's key: through (binds it)", r.ok ? `+${r.points}` : r.reason);
await sleep(1200);
p.send({ t: "claim", node: 0, via: a.ticket, far: { region: Y, key: c.keys[0] } });
r = await p.next("claim");
check(r.ok || r.reason === "linked", "again from A: still through", r.ok ? `+${r.points}` : r.reason);
await p.at(b.node);
await sleep(1200);
p.send({ t: "claim", node: 0, via: b.ticket, far: { region: Y, key: c.keys[0] } });
r = await p.next("claim");
check(!r.ok && r.reason === "nokey", "the same key from B: nokey (bound to A's link)", r.reason);
p.close();

console.log(failed ? `${failed} FAILED` : "all ok");
process.exit(failed ? 1 : 0);
