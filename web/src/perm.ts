// Permanent sources (Dauerquelle: Glut / Eis; server: keep.rs). The app holds
// one bearer ticket per source and region; the server stores only hashes of
// them. A player has a few slots (more by level). One button next to
// "Erobern": make an own fresh claim permanent while a slot is free, else
// move one of them here (a card picks which, the weakest first), or top one
// up when standing next to it. Tickets are redeemed on connect and every few
// minutes, one at a time (points for standing, or for a fallen source).
import { t, type Key } from "./i18n";
import type { Faction, Net, ServerMsg } from "./net";
import type { MottoItem } from "./protocol";
import { $, plus } from "./page";
import { region } from "./region";
import { loadJson, saveJson } from "./store";
import { ask } from "./ask";
import { mottoText, setMottos } from "./mottos";

const KEY = () => `elements.perm.${region}` as const;
const REDEEM_EVERY_MS = 10 * 60_000;
/** The server takes one redeem per player every 20 s. */
const REDEEM_GAP_MS = 25_000;

type Stored = { ticket: string; node: number; ll?: [number, number] };

const valid = (v: Stored) => typeof v?.ticket === "string" && typeof v?.node === "number";

function load(): Stored[] {
  const v = loadJson<Stored | Stored[]>(KEY());
  // Before several: one object.
  const list = Array.isArray(v) ? v : v ? [v] : [];
  return list.filter(valid);
}

function save(list: Stored[]) {
  saveJson(KEY(), list.length ? list : null);
}

/** A source as main.ts knows it (the fields used here). */
export type PermSrc = {
  node: number;
  faction: Faction;
  mine: boolean;
  ll: [number, number];
  kind: string;
  until: number;
  full: number;
  nokeep?: boolean;
  motto?: string | null;
  sieged?: boolean;
};

export type PermMsg = Extract<ServerMsg, { t: "keep" | "kept" | "released" | "motto" }>;

export function setupPerm(
  net: Net,
  toast: (s: string, ms?: number) => void,
  distM: (a: [number, number], b: [number, number]) => number,
  radiusM: number,
  /** Show a source on the map (after placing, or from the list). */
  focus: (node: number) => void,
  /** The player's level (1 without levels). */
  level: () => number,
  /** The player's badges (some tag lines need one). */
  badges: () => string[],
) {
  const btn = document.getElementById("perm") as HTMLButtonElement;
  let on = false;
  /** Slots in use on the server (also by tickets this device lacks). */
  let used = 0;
  let base = 1;
  let perLevel = 0;
  let gap = 60;
  let faction: Faction | null = null;
  let mine = load();
  let action: { node: number; kind: "place" | "move" | "topup"; ticket?: string } | null = null;
  let redeemTimers: number[] = [];
  let redeemEvery = 0;
  /** The ticket of a move in flight (its source goes when the new one comes). */
  let moving: string | null = null;
  let lastSources: PermSrc[] = [];
  let lastMe: [number, number] | null = null;
  let mottos: MottoItem[] = [];
  /** Nodes whose source was in a ring at the last update. */
  let sieged = new Set<number>();
  let levels = false;
  /** Tag lines just chosen, until the source list shows them. */
  const chosen = new Map<number, string>();
  /** Own sources' tag lines as last seen on the map (kept when out of view). */
  const shownMotto = new Map<number, string>();

  const name = (k: string) => t(`perm.${k}.${faction ?? "fire"}` as Key);
  const max = () => base + perLevel * Math.max(0, level() - 1);
  const nodes = () => new Set(mine.map((m) => m.node));

  function redeemAll() {
    redeemTimers.forEach(clearTimeout);
    redeemTimers = mine.map((m, i) => window.setTimeout(() => net.send({ t: "redeem", ticket: m.ticket }), i * REDEEM_GAP_MS));
  }

  /** Strength of an own source (0..1) if its tile is loaded. */
  function strength(m: Stored): number | null {
    const s = lastSources.find((x) => x.node === m.node && x.kind === "keep");
    return s ? Math.min(1, Math.max(0, (s.until - Date.now()) / (s.full * 1000))) : null;
  }

  function where(m: Stored): [number, number] | undefined {
    return lastSources.find((x) => x.node === m.node)?.ll ?? m.ll;
  }

  /** Own sources, weakest first (unknown strength last). */
  function ranked() {
    return mine
      .map((m) => ({ m, s: strength(m), d: lastMe && where(m) ? distM(lastMe, where(m)!) : null }))
      .sort((a, b) => (a.s ?? 2) - (b.s ?? 2));
  }

  function line(r: { s: number | null; d: number | null }) {
    const parts = [];
    if (r.d !== null) parts.push(t("perm.dist", { m: r.d < 1000 ? Math.round(r.d) : `${(r.d / 1000).toFixed(1)} k` }));
    if (r.s !== null) parts.push(t("perm.strength", { p: Math.round(r.s * 100) }));
    return parts.join(" · ");
  }

  // Which one to move here: a card listing them, the weakest marked.
  function pick(spot: number) {
    const box = $("perm-pick");
    $("perm-pick-title").textContent = name("pick");
    const list = $("perm-pick-list");
    list.replaceChildren(
      ...ranked().map((r, i) => {
        const b = document.createElement("button");
        b.className = i === 0 ? "weakest" : "";
        b.textContent = `${name("one")} ${i + 1}` + (line(r) ? ` · ${line(r)}` : "") + (i === 0 ? ` · ${t("perm.weakest")}` : "");
        b.onclick = () => {
          box.hidden = true;
          moving = r.m.ticket;
          net.send({ t: "keep", node: spot, ticket: r.m.ticket });
        };
        return b;
      }),
    );
    $("perm-pick-no").onclick = () => (box.hidden = true);
    box.hidden = false;
  }

  // Tag lines: the side's, unlocked by level (without levels: those of
  // level 2) and badges; locked ones shown greyed with what unlocks them.
  function mottoPicker(m: Stored) {
    const sel = document.createElement("select");
    sel.className = "motto";
    const lvl = levels ? level() : 2;
    const have = badges();
    const none = new Option(t("perm.motto.none"), "");
    sel.append(none);
    // The city's campaign phrases first, in their own group (open to all).
    const city = mottos.filter((x) => x.side === faction && x.names);
    if (city.length) {
      const g = document.createElement("optgroup");
      g.label = t("perm.motto.city");
      g.append(...city.map((x) => new Option(mottoText(x.key), x.key)));
      sel.append(g);
    }
    for (const x of mottos.filter((x) => x.side === faction && !x.names)) {
      const open = lvl >= x.level && (!x.badge || have.some((b) => b.startsWith(x.badge!)));
      const why = x.badge ? t(x.badge === "coll:" ? "perm.motto.coll" : "perm.motto.win") : t("perm.motto.level", { n: x.level });
      const o = new Option(open ? mottoText(x.key) : `${mottoText(x.key)} (${why})`, x.key);
      o.disabled = !open;
      sel.append(o);
    }
    sel.value = chosen.get(m.node) ?? shownMotto.get(m.node) ?? "";
    sel.onchange = () => {
      chosen.set(m.node, sel.value);
      net.send({ t: "motto", node: m.node, ticket: m.ticket, motto: sel.value || null });
    };
    return sel;
  }

  /** May this player give a tag line now (level 2, or the city's phrases)? */
  const mottoOpen = () => mottos.length > 0 && (!levels || level() >= 2 || mottos.some((x) => x.names && x.side === faction));

  // Right after placing: offer the tag line at once (else it hides in the sheet).
  function offerMotto(m: Stored) {
    if (!mottoOpen()) return;
    const card = $("motto-pick");
    $("motto-pick-title").textContent = t(`perm.motto.ask.${faction ?? "fire"}` as Key);
    const sel = mottoPicker(m);
    const set = sel.onchange;
    sel.onchange = (e) => {
      set?.call(sel, e);
      card.hidden = true;
    };
    $("motto-pick-body").replaceChildren(sel);
    $("motto-pick-no").onclick = () => (card.hidden = true);
    card.hidden = false;
  }

  // The list in the player sheet: tap one to see it on the map.
  function drawList() {
    const el = $("my-keeps");
    el.hidden = !on || (!mine.length && !used);
    if (el.hidden) return;
    el.querySelector("h3")!.textContent = t(`perm.mine.${faction ?? "fire"}` as Key, { n: Math.max(used, mine.length), max: max() });
    el.querySelector("ul")!.replaceChildren(
      ...ranked().map((r, i) => {
        const li = document.createElement("li");
        const b = document.createElement("button");
        b.textContent = `${name("one")} ${i + 1}` + (line(r) ? ` · ${line(r)}` : "");
        b.onclick = () => {
          ($("board-close") as HTMLButtonElement).click();
          focus(r.m.node);
        };
        li.append(b);
        // A tag line from level 2 (without levels: always); the city's for everyone.
        if (mottoOpen()) {
          const lab = document.createElement("label");
          lab.className = "motto-row";
          lab.append(t("perm.motto"), " ", mottoPicker(r.m));
          li.append(lab);
        }
        return li;
      }),
    );
  }

  btn.onclick = () => {
    if (!action) return;
    if (action.kind === "move") return pick(action.node);
    moving = null;
    net.send({ t: "keep", node: action.node, ...(action.ticket ? { ticket: action.ticket } : {}) });
  };

  function forget(ticket: string) {
    mine = mine.filter((m) => m.ticket !== ticket);
    save(mine);
  }

  return {
    /** The nodes of this device's permanent sources here. */
    nodes,
    drawList,
    onWelcome(
      m: { keep?: boolean; keep_used?: number; keep_base?: number; keep_per_level?: number; keep_gap_m?: number; mottos?: MottoItem[]; levels?: boolean },
      f: Faction,
    ) {
      mottos = m.mottos ?? [];
      setMottos(mottos);
      levels = !!m.levels;
      on = !!m.keep;
      used = m.keep_used ?? 0;
      base = m.keep_base ?? 1;
      perLevel = m.keep_per_level ?? 0;
      gap = m.keep_gap_m ?? 60;
      faction = f;
      redeemTimers.forEach(clearTimeout);
      clearInterval(redeemEvery);
      if (!on) {
        btn.hidden = true;
        return drawList();
      }
      redeemAll();
      redeemEvery = window.setInterval(redeemAll, REDEEM_EVERY_MS);
      drawList();
    },
    onMsg(m: PermMsg) {
      if (m.t === "keep") {
        const moved = moving;
        moving = null;
        if (m.ok && m.ticket) {
          const spot = lastSources.find((s) => s.node === m.node);
          if (moved) forget(moved);
          else used++;
          const placed = { ticket: m.ticket, node: m.node, ...(spot ? { ll: spot.ll } : {}) };
          mine.push(placed);
          save(mine);
          toast(name(moved ? "moved" : "placed"), 6000);
          focus(m.node);
          offerMotto(placed);
        } else if (m.ok && m.topup) {
          toast(name("topped"));
        } else if (m.reason === "slot") {
          // Slots taken by tickets this device doesn't have (new phone, cleared browser).
          void ask(name("release.confirm")).then((yes) => yes && net.send({ t: "release", tickets: mine.map((x) => x.ticket) }));
        } else {
          const why: Record<string, string> = {
            near: "perm.near",
            range: "claim.range",
            cooldown: "claim.cooldown",
            moving: "perm.moving",
            notclaim: "perm.notclaim",
          };
          toast(t((why[m.reason ?? ""] ?? "perm.failed") as Key, { m: gap }));
        }
      } else if (m.t === "kept") {
        const own = mine.find((x) => x.ticket === m.ticket);
        if (m.state === "standing") {
          if (own && m.node !== undefined && own.node !== m.node) {
            own.node = m.node;
            save(mine);
          }
          if (m.points > 0) toast(name("points") + plus(m.points), 5000);
        } else {
          if (m.state === "fallen") toast(name("fallen") + plus(m.points), 7000);
          if (own) {
            forget(m.ticket);
            used = Math.max(0, used - 1);
          }
        }
      } else if (m.t === "motto") {
        if (!m.ok) chosen.delete(m.node);
        toast(t(m.ok ? "perm.motto.set" : m.reason === "locked" ? "perm.motto.locked" : "perm.failed"));
      } else if (m.t === "released") {
        if (m.ok) {
          used = mine.length;
          toast(t("perm.released"));
        } else toast(t("perm.release.wait"));
      }
      drawList();
    },
    /** Button state from the position and the sources around. */
    update(me: [number, number] | null, sources: PermSrc[], f: Faction | null) {
      faction = f;
      // Own permanent source newly in a ring: tell the owner once.
      for (const s of sources) {
        const own = s.kind === "keep" && mine.some((m) => m.node === s.node);
        if (own && s.sieged && !sieged.has(s.node)) toast(t(`perm.sieged.${s.faction}` as Key), 7000);
      }
      sieged = new Set(sources.filter((s) => s.sieged).map((s) => s.node));
      lastSources = sources;
      lastMe = me;
      for (const [node, v] of chosen) if ((sources.find((s) => s.node === node)?.motto ?? "") === v) chosen.delete(node);
      // The list is drawn before the sources come: redraw when a tag line shows up or changes.
      let changed = false;
      for (const s of sources) {
        if (s.kind !== "keep" || !mine.some((m) => m.node === s.node)) continue;
        const v = s.motto ?? "";
        if (shownMotto.get(s.node) !== v) {
          shownMotto.set(s.node, v);
          changed = true;
        }
      }
      if (changed) drawList();
      action = null;
      let label = "";
      if (on && me && f) {
        const near = (s: PermSrc) => distM(me, s.ll) <= radiusM;
        const own = mine
          .map((m) => ({ m, s: sources.find((s) => s.node === m.node && s.kind === "keep") }))
          .find((x) => x.s && near(x.s));
        if (own?.s && own.s.until - Date.now() < own.s.full * 500) {
          // Own permanent source in reach and half faded: top it up.
          action = { node: own.s.node, kind: "topup", ticket: own.m.ticket };
        } else if (!own) {
          // The latest own claim in reach (the one just made, not an older
          // one nearer by), away from other permanent sources.
          const keeps = sources.filter((s) => s.kind === "keep");
          const spot = sources
            // Not on or next to a natural source (the server says which: `nokeep`).
            .filter((s) => s.mine && s.faction === f && (s.kind === "" || s.kind === "cut") && !s.nokeep && near(s))
            .filter((s) => keeps.every((k) => distM(k.ll, s.ll) >= gap))
            .sort((a, b) => b.until - a.until)[0];
          const free = max() - Math.max(used, mine.length);
          if (spot && free > 0) {
            action = { node: spot.node, kind: "place" };
            label = `${name("place")} (${t("perm.free", { n: free })})`;
          } else if (spot && mine.length) action = { node: spot.node, kind: "move" };
          else if (spot) action = { node: spot.node, kind: "place" }; // the server asks to free a slot
        }
      }
      btn.hidden = !action;
      if (action) {
        btn.className = faction ?? "";
        btn.textContent = label || name(action.kind);
      }
    },
  };
}
