// The claim and rate buttons (web.rs `Claim`, `Rate`): what the claim button
// takes (`target`), the wait after a claim or while moving, the answer's toast.
import { t, type Key } from "./i18n";
import type { Faction, Net, ServerMsg } from "./net";
import type { Sound } from "./sound";
import type { Src } from "./sources";
import { arrow, CLAIM_RADIUS_M, distM, owner } from "./geo";
import { $, plus, toast } from "./page";

const TOO_FAST = t("too.fast");

type Target = { id: number; anchor: boolean; reinforce?: boolean };
/** Farther than this the front hint is no help on foot. */
const FRONT_HINT_M = 2000;

export function setupClaim(d: {
  net: Net;
  sound: Sound;
  nodePos: Map<number, [number, number]>;
  state: Map<number, [number, number]>;
  sources: () => Src[];
  faction: () => Faction | null;
  me: () => [number, number] | null;
  claimRadius: () => number;
  supply: () => number | null;
  reinforce: () => boolean;
  perm: { update(me: [number, number] | null, sources: Src[], faction: Faction | null): void };
  /** The tunnel's say on a claim: true if it went through one. */
  tunnel: { onClaim(node: number, ok: boolean): boolean };
  card: { onClaim(node: number): void };
}) {
  const { net, sound, nodePos, state } = d;
  const claimBtn = $("claim") as HTMLButtonElement;
  const rateBtn = $("rate") as HTMLButtonElement;
  // Wait after a claim (cooldown) or while moving too fast: the button is
  // off, fills up and counts down.
  let wait: { from: number; until: number; moving: boolean } | null = null;
  const drawWait = () => {
    const now = Date.now();
    const faction = d.faction();
    const me = d.me();
    if (wait && now >= wait.until) {
      if (!wait.moving) sound.tick();
      wait = null;
    }
    claimBtn.classList.toggle("waiting", !!wait);
    if (wait) {
      const p = (now - wait.from) / Math.max(1, wait.until - wait.from);
      claimBtn.style.setProperty("--p", `${Math.round(p * 100)}%`);
      claimBtn.textContent = t(wait.moving ? "claim.wait.moving" : "claim.wait", { s: Math.ceil((wait.until - now) / 1000) });
    } else {
      // Nothing to take nearby, but an anchor spot or the front to reinforce: the button says so.
      const tg = me ? target(me) : null;
      claimBtn.textContent = tg?.anchor
        ? t(faction === "fire" ? "claim.anchor.go.fire" : "claim.anchor.go.water")
        : tg?.reinforce
          ? t("claim.reinforce.go")
          : t("claim.button");
    }
    claimBtn.disabled = !(faction && me) || !!wait;
  };
  setInterval(() => wait && drawWait(), 200);
  const update = () => {
    const faction = d.faction();
    const me = d.me();
    d.perm.update(me, d.sources(), faction);
    drawWait();
    claimBtn.disabled = !(faction && me) || !!wait;
    rateBtn.disabled = !(faction && me);
  };

  /** Where the last successful claim went (for the video script). */
  let lastClaim: [number, number] | null = null;
  // A claim always gets an answer; without one the connection is renewed and
  // the player is told to tap again (it used to fail silently after sleep).
  let claimWait = 0;
  function send(node: number) {
    if (!net.send({ t: "claim", node })) return toast(t("connection.retry"));
    net.check();
    clearTimeout(claimWait);
    claimWait = window.setTimeout(() => toast(t("connection.retry")), 5_000);
  }

  /** The last claim sent was a reinforcement; where the server found no front. */
  let reinforcing = false;
  let noFront: [number, number] | null = null;

  // What the claim button takes: a spark of the other element in range
  // (even while its junction still looks like ours), else the nearest node in
  // range that isn't ours yet, else (with supply) an anchor on own ground: the
  // nearest junction without a source and away from our anchors (the server
  // measures along the streets and may still say "supplied"), else (with
  // reinforcing) own ground without a source closest to ground that isn't
  // ours (the server checks the street neighbours).
  function target(p: [number, number]): Target | null {
    const faction = d.faction();
    const sources = d.sources();
    const supply = d.supply();
    const mine = faction === "fire" ? 1 : 2;
    const nearest = (ok: (id: number) => boolean): number | null => {
      let best: [number, number] | null = null;
      for (const [id, c] of nodePos) {
        const dist = distM(p, c);
        if (dist > d.claimRadius() || (best && dist >= best[1]) || !ok(id)) continue;
        best = [id, dist];
      }
      return best?.[0] ?? null;
    };
    const src = new Map(sources.map((s) => [s.node, s]));
    const spark = nearest((id) => src.get(id)?.kind === "spark" && src.get(id)?.faction !== faction);
    if (spark !== null) return { id: spark, anchor: false };
    const open = nearest((id) => {
      const [f, w] = state.get(id) ?? [0, 0];
      return owner(f, w) !== mine && !(src.get(id)?.faction === faction);
    });
    if (open !== null) return { id: open, anchor: false };
    if (supply === null) return null;
    const anchors = sources.filter((s) => s.kind === "anchor" && s.faction === faction).map((s) => s.ll);
    const spot = nearest((id) => {
      const c = nodePos.get(id)!;
      return !src.has(id) && anchors.every((a) => distM(a, c) > supply);
    });
    if (spot !== null) return { id: spot, anchor: true };
    return d.reinforce() ? front(p, mine, src) : null;
  }

  /** Own junction without a source in range, nearest first (the server
   * finds the front from there); not where the server just found none. */
  function front(p: [number, number], mine: number, src: Map<number, Src>): Target | null {
    if (noFront && distM(noFront, p) < 30) return null;
    let best: [number, number] | null = null;
    for (const [id, c] of nodePos) {
      const dist = distM(p, c);
      if (src.has(id) || dist > d.claimRadius() || (best && dist >= best[1])) continue;
      const [f, w] = state.get(id) ?? [0, 0];
      if (owner(f, w) === mine) best = [id, dist];
    }
    return best ? { id: best[0], anchor: false, reinforce: true } : null;
  }

  // Why the button has nothing to take: an anchor too close (supply), or
  // really all ours; plus the way to the nearest junction that isn't ours.
  function nothing(p: [number, number]): string {
    const faction = d.faction();
    const mine = faction === "fire" ? 1 : 2;
    const src = new Set(d.sources().map((s) => s.node));
    const anchors = d.sources().filter((s) => s.kind === "anchor" && s.faction === faction);
    let free = false;
    let front: [[number, number], number] | null = null;
    for (const [id, c] of nodePos) {
      const dist = distM(p, c);
      if (dist <= d.claimRadius() && !src.has(id)) free = true;
      const [f, w] = state.get(id) ?? [0, 0];
      if (owner(f, w) !== mine && (!front || dist < front[1])) front = [c, dist];
    }
    let msg = t("claim.allmine");
    if (free && anchors.length) {
      const m = Math.min(...anchors.map((a) => distM(p, a.ll)));
      msg = t(faction === "water" ? "claim.anchornear.water" : "claim.anchornear.fire", { m: Math.round(m / 10) * 10 });
    }
    if (front && front[1] <= FRONT_HINT_M) {
      msg += ` · ${t("claim.front", { d: `${Math.round(front[1] / 10) * 10} m ${arrow(p, front[0])}` })}`;
    }
    return msg;
  }

  const sheet = $("sheet");
  rateBtn.onclick = () => (sheet.hidden = !sheet.hidden);
  for (const b of sheet.querySelectorAll<HTMLButtonElement>("button[data-cat]")) {
    b.onclick = () => {
      sheet.hidden = true;
      const me = d.me();
      if (!me) return;
      let best: [number, number] | null = null;
      for (const [id, c] of nodePos) {
        const dist = distM(me, c);
        if (!best || dist < best[1]) best = [id, dist];
      }
      if (!best || best[1] > CLAIM_RADIUS_M) return toast(t("rate.noplace"));
      net.send({ t: "rate", node: best[0], cat: b.dataset.cat! });
    };
  }

  claimBtn.onclick = () => {
    const me = d.me();
    if (!me) return;
    const tg = target(me);
    if (tg === null) return toast(nothing(me));
    reinforcing = !!tg.reinforce;
    send(tg.id);
  };

  return {
    update,
    send,
    /** Seconds left of the button's wait (cooldown or moving), if any. */
    wait: () => (wait && Date.now() < wait.until ? { s: Math.ceil((wait.until - Date.now()) / 1000), moving: wait.moving } : null),
    target,
    lastClaim: () => lastClaim,
    onRate(m: Extract<ServerMsg, { t: "rate" }>) {
      const why = m.reason === "cooldown" ? t("rate.cooldown") : m.reason === "moving" ? TOO_FAST : t("rate.nothere");
      toast(m.ok ? t("rate.thanks", { plus: plus(m.points) }) : why);
    },
    onClaim(m: Extract<ServerMsg, { t: "claim" }>) {
      const faction = d.faction();
      clearTimeout(claimWait);
      if (m.ok) lastClaim = nodePos.get(m.node) ?? null;
      if (m.wait && m.wait > 0.3) {
        const now = Date.now();
        wait = { from: now, until: now + m.wait * 1000, moving: m.reason === "moving" };
        drawWait();
      }
      const why: Record<string, Key> = {
        range: "claim.range",
        cooldown: "claim.cooldown",
        own: "claim.own",
        strong: "claim.strong",
        supplied: "claim.supplied",
        worn: "claim.worn",
      };
      // On enemy ground a claim builds up first (rule `claim_ramp_s`).
      const via = d.tunnel.onClaim(m.node, m.ok);
      if (m.ok && faction) {
        if (m.doused) sound.steam();
        else sound.claim(faction);
        if (m.build && m.build >= 2) sound.building(faction, m.build);
      }
      d.card.onClaim(m.node);
      let msg = m.ok
        ? via
          ? t("tunnel.ok", { plus: plus(m.points) })
          : m.doused
          ? t(faction === "fire" ? "claim.doused.water" : "claim.doused.fire", { plus: plus(m.points) })
          : m.anchor
          ? t(faction === "fire" ? "claim.anchor.fire" : "claim.anchor.water", { plus: plus(m.points) })
          : reinforcing
          ? t("claim.reinforced")
          : m.build && m.build >= 2
          ? t("claim.building", { plus: plus(m.points), s: Math.round(m.build) })
          : t(m.bonus ? "claim.bonus" : "claim.ok", { plus: plus(m.points) })
        : m.reason === "moving"
          ? TOO_FAST
          : t(why[m.reason ?? ""] ?? "claim.failed");
      // No front within reach here: say so instead of offering it again.
      if (reinforcing && !m.ok && m.reason === "supplied") {
        noFront = d.me();
        const me = d.me();
        if (me) msg = nothing(me);
        drawWait();
      }
      reinforcing = false;
      toast(msg);
    },
  };
}
