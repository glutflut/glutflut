// The claim button (web.rs `Claim`): what it takes (`target`), the wait after a claim or while moving, the answer's toast.
import { t, type Key } from "./i18n";
import type { Faction, Net, ServerMsg } from "./net";
import type { Sound } from "./sound";
import type { Src } from "./sources";
import type { Link } from "./perm";
import type { Far } from "./protocol";
import { arrow, CLAIM_RADIUS_M, distM, owner } from "./geo";
import { $, plus, toast } from "./page";

const TOO_FAST = t("too.fast");
/** Why a claim (or a link's offer) came to nothing. */
const WHY: Record<string, Key> = {
  range: "claim.range",
  cooldown: "claim.cooldown",
  own: "claim.own",
  strong: "claim.strong",
  supplied: "claim.supplied",
  worn: "claim.worn",
  linked: "claim.linked",
  linkfar: "claim.linkfar",
  linkdown: "claim.linkdown",
  nokey: "claim.nokey",
};

type Target = { id: number; anchor: boolean; reinforce?: boolean };
/** Ask the server again what the link would take, at most this often while in reach. */
const PEEK_EVERY_MS = 15_000;
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
  /** Own linked permanent sources (perm.ts). */
  links: () => Link[];
  /** Show the camera a spot (a claim through a link), then back to the player. */
  look: (ll: [number, number]) => void;
  /** A claim through a link landed in another region: show that place small. */
  farShown: (
    region: string,
    name: string,
    at: [number, number],
    near: [number, number, number, number][],
    streets: [number, number][],
    from: [number, number] | null,
  ) => void;
  /** A claim went through the link of own source A (to B here, or null: another region). */
  fired: (a: [number, number], b: [number, number] | null) => void;
  /** The tunnel's say on a claim: true if it went through one. */
  tunnel: { onClaim(node: number, ok: boolean): boolean };
  card: { onClaim(node: number): void };
  /** A claim of this player took (invite nudge). */
  claimed: () => void;
  /** "Wie ist es hier?" asks now and then after a claim (rate.ts). */
  rate: { onClaimed(): void };
}) {
  const { net, sound, nodePos, state } = d;
  const claimBtn = $("claim") as HTMLButtonElement;
  const linkBtn = $("claim-link") as HTMLButtonElement;
  const rateBtn = $("rate") as HTMLButtonElement;
  // The second button: standing at an own linked permanent source, what a
  // claim through the link would take (the server says, `peek`).
  let linkTarget: { ticket: string; node: number; ll: [number, number]; far?: Far & { name?: string } } | null = null;
  /** Standing at a linked source with nothing offered: why (the button stays, dimmed). */
  let linkWhy: string | null = null;
  let peeked: { ticket: string; at: number; far?: Far & { name?: string } } = { ticket: "", at: 0 };
  /** Where the claim just sent through a link goes (the camera shows it when it lands). */
  let flyTo: [number, number] | null = null;
  /** The link a claim just went through (until the answer). */
  let fired: Link | null = null;
  /** A claim through a link is on its way (a far region may wake first): the button waits. */
  let linkBusy = 0;
  /** The region name of the far link just used (for the answer's toast). */
  let farName = "";
  let farRegion = "";
  function drawLink() {
    const faction = d.faction();
    linkBtn.hidden = !(linkTarget || linkWhy) || !faction;
    linkBtn.classList.toggle("idle", !linkTarget);
    linkBtn.className = faction ?? "";
    linkBtn.classList.toggle("idle", !linkTarget);
    linkBtn.textContent = linkBusy ? t("claim.link.busy") : t("claim.link");
    linkBtn.disabled = !!wait || !!linkBusy;
  }
  function peek() {
    const me = d.me();
    // The server's radius around A, not the armed reach (that counts from B).
    const l = me && d.faction() ? d.links().find((l) => distM(me, l.ll) <= CLAIM_RADIUS_M) : undefined;
    if (!l) {
      linkTarget = null;
      linkWhy = null;
      peeked = { ticket: "", at: 0 };
    } else if (peeked.ticket !== l.ticket || Date.now() - peeked.at > PEEK_EVERY_MS) {
      const far = l.far && { region: l.far.region, key: l.far.key };
      peeked = { ticket: l.ticket, at: Date.now(), far: l.far };
      net.send({ t: "peek", via: l.ticket, ...(far ? { far } : {}) });
    }
    drawLink();
  }
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
      let label = tg?.anchor
        ? t(faction === "fire" ? "claim.anchor.go.fire" : "claim.anchor.go.water")
        : tg?.reinforce
          ? t("claim.reinforce.go")
          : t("claim.button");
      // Nothing at all in range: the button points the way (the first tip).
      if (!tg && me && faction) {
        const f = nearestFront(me, faction === "fire" ? 1 : 2);
        if (f && f[1] > d.claimRadius() && f[1] <= FRONT_HINT_M) label = t("claim.front", { d: `${Math.round(f[1] / 10) * 10} m ${arrow(me, f[0])}` });
      }
      claimBtn.textContent = label;
    }
    claimBtn.disabled = !(faction && me) || !!wait;
    linkBtn.disabled = !!wait || !!linkBusy;
  };
  setInterval(() => wait && drawWait(), 200);
  const update = () => {
    const faction = d.faction();
    const me = d.me();
    d.perm.update(me, d.sources(), faction);
    drawWait();
    claimBtn.disabled = !(faction && me) || !!wait;
    rateBtn.disabled = !(faction && me);
    peek();
  };
  linkBtn.onclick = () => {
    if (!linkTarget) {
      if (linkWhy) toast(t(WHY[linkWhy] ?? "claim.linked"));
      return;
    }
    flyTo = linkTarget.ll;
    const tk = linkTarget.ticket;
    fired = d.links().find((l) => l.ticket === tk) ?? null;
    farName = linkTarget.far?.name ?? "";
    farRegion = linkTarget.far?.region ?? "";
    // Until the answer (or the app's own timeout below) the button waits.
    clearTimeout(linkBusy);
    linkBusy = window.setTimeout(
      () => {
        linkBusy = 0;
        drawLink();
      },
      linkTarget.far ? 95_000 : 6_000,
    );
    drawLink();
    send(linkTarget.node, linkTarget.ticket, linkTarget.far);
  };

  /** Where the last successful claim went (for the video script). */
  let lastClaim: [number, number] | null = null;
  // A claim always gets an answer; without one the connection is renewed and
  // the player is told to tap again (it used to fail silently after sleep).
  let claimWait = 0;
  let waking = 0;
  function send(node: number, via?: string, far?: Far) {
    const f = far && { region: far.region, key: far.key };
    if (!net.send({ t: "claim", node, ...(via ? { via } : {}), ...(f ? { far: f } : {}) })) return toast(t("connection.retry"));
    net.check();
    clearTimeout(claimWait);
    // Into another region that may sleep: it loads first (up to ~1.5 min).
    if (far) {
      waking = window.setTimeout(() => toast(t("claim.waking"), 90_000), 2_500);
      claimWait = window.setTimeout(() => toast(t("claim.linkdown")), 95_000);
    } else claimWait = window.setTimeout(() => toast(t("connection.retry")), 5_000);
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
    const takeable = (id: number) => {
      const [f, w] = state.get(id) ?? [0, 0];
      return owner(f, w) !== mine && !(src.get(id)?.faction === faction);
    };
    const open = nearest(takeable);
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

  /** The nearest junction that isn't ours, at any distance, and how far. */
  function nearestFront(p: [number, number], mine: number): [[number, number], number] | null {
    let front: [[number, number], number] | null = null;
    for (const [id, c] of nodePos) {
      const dist = distM(p, c);
      const [f, w] = state.get(id) ?? [0, 0];
      if (owner(f, w) !== mine && (!front || dist < front[1])) front = [c, dist];
    }
    return front;
  }

  // Why the button has nothing to take: no junction in range at all, an
  // anchor too close (supply), or really all ours; plus the way to the
  // nearest junction that isn't ours.
  function nothing(p: [number, number]): string {
    const faction = d.faction();
    const mine = faction === "fire" ? 1 : 2;
    const src = new Set(d.sources().map((s) => s.node));
    const anchors = d.sources().filter((s) => s.kind === "anchor" && s.faction === faction);
    let free = false;
    let any = false;
    for (const [id, c] of nodePos) {
      const dist = distM(p, c);
      if (dist <= d.claimRadius()) {
        any = true;
        if (!src.has(id)) free = true;
      }
    }
    const front = nearestFront(p, mine);
    let msg = t(any ? "claim.allmine" : "claim.nonear");
    if (free && anchors.length) {
      const m = Math.min(...anchors.map((a) => distM(p, a.ll)));
      msg = t(faction === "water" ? "claim.anchornear.water" : "claim.anchornear.fire", { m: Math.round(m / 10) * 10 });
    }
    if (front && front[1] <= FRONT_HINT_M) {
      msg += ` · ${t("claim.front", { d: `${Math.round(front[1] / 10) * 10} m ${arrow(p, front[0])}` })}`;
    }
    return msg;
  }

  claimBtn.onclick = () => {
    const me = d.me();
    if (!me) return;
    const tg = target(me);
    // Nothing left here, but the link has a target: through the link.
    if (tg === null && linkTarget && !linkBtn.hidden) return linkBtn.click();
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
    /** The server's answer to `peek`: the second button and where it goes. */
    onPeek(m: Extract<ServerMsg, { t: "peek" }>) {
      if (m.via !== peeked.ticket) return;
      linkTarget =
        m.node !== undefined && m.lon !== undefined && m.lat !== undefined
          ? { ticket: m.via, node: m.node, ll: [m.lon, m.lat], far: peeked.far }
          : null;
      // The far region sleeps: nothing known, but a claim wakes it.
      if (!linkTarget && m.reason === "asleep" && peeked.far) {
        const l = d.links().find((x) => x.ticket === m.via);
        if (l) linkTarget = { ticket: m.via, node: 0, ll: l.partner, far: peeked.far };
      }
      linkWhy = linkTarget ? null : (m.reason ?? null);
      drawLink();
    },
    onClaim(m: Extract<ServerMsg, { t: "claim" }>) {
      const faction = d.faction();
      clearTimeout(claimWait);
      clearTimeout(waking);
      clearTimeout(linkBusy);
      linkBusy = 0;
      // Through a link: show where it landed, and ask again what is left there.
      if (flyTo) {
        // The server picks the junction around B; the peeked one is the fallback.
        // In another region the map can't go there.
        if (m.ok && !m.far) d.look(nodePos.get(m.node) ?? flyTo);
        if (m.ok && m.far && farRegion) d.farShown(farRegion, farName, m.far, m.near ?? [], m.streets ?? [], m.from ?? null);
        if (m.ok && fired) d.fired(fired.ll, fired.far ? null : (m.from ?? fired.partner));
        fired = null;
        flyTo = null;
        linkTarget = null;
        peeked.at = 0;
        drawLink();
      }
      if (m.ok && !m.far) lastClaim = nodePos.get(m.node) ?? null;
      if (m.wait && m.wait > 0.3) {
        const now = Date.now();
        wait = { from: now, until: now + m.wait * 1000, moving: m.reason === "moving" };
        drawWait();
      }
      const why = WHY;
      // On enemy ground a claim builds up first (rule `claim_ramp_s`).
      // A node of another region means nothing to the map here.
      const via = !m.far && d.tunnel.onClaim(m.node, m.ok);
      if (m.ok && faction) {
        d.claimed();
        if (!m.far) d.rate.onClaimed();
        if (m.doused) sound.steam();
        else sound.claim(faction);
        if (m.build && m.build >= 2) sound.building(faction, m.build);
      }
      if (!m.far) d.card.onClaim(m.node);
      let msg = m.ok
        ? m.far
          ? t("claim.far.ok", { region: farName, plus: plus(m.points) })
          : via
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
