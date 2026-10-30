// Permanent sources (Dauerquelle: Glut / Eis; server: keep.rs). The app holds
// one bearer ticket per source and region; the server stores only hashes of
// them. A player has a few slots (more by level). One button next to
// "Erobern": make an own fresh claim permanent while a slot is free, else
// move one of them here (a card picks which, the weakest first), or top one
// up when standing next to it. Tickets are redeemed on connect and every few
// minutes, one at a time (points for standing, or for a fallen source).
import type * as maplibregl from "maplibre-gl";
import { t, type Key } from "./i18n";
import { COLORS } from "./geo";
import type { Faction, Net, ServerMsg } from "./net";
import type { MottoItem } from "./protocol";
import { $, plus } from "./page";
import { listRegions, openRegion, region } from "./region";
import { loadJson, saveJson, type StoreKey } from "./store";
import { ask } from "./ask";
import { mottoText, setMottos } from "./mottos";
import { arrivedGift, gifts, showGift } from "./gift";

const KEY = () => `elements.perm.${region}` as const;
const REDEEM_EVERY_MS = 10 * 60_000;
/** The server takes one redeem per player every 20 s. */
const REDEEM_GAP_MS = 25_000;

/** `link`: the partner's ticket (linked permanent sources, server `[keep] link`).
 * `far`: the partner in another region of this server (only this device knows it). */
type Stored = { ticket: string; node: number; ll?: [number, number]; link?: string; far?: FarEnd };
/** The far end of a link: its region, its ticket (the key), where it is and the region's name. */
type FarEnd = { region: string; key: string; ll?: [number, number]; name?: string };

const valid = (v: Stored) => typeof v?.ticket === "string" && typeof v?.node === "number";

function load(): Stored[] {
  const v = loadJson<Stored | Stored[]>(KEY());
  // Before several: one object.
  const list = (Array.isArray(v) ? v : v ? [v] : []).filter(valid);
  // One link per source: a link here (the server's) wins over a far one
  // (an earlier version could set both).
  const both = list.filter((m) => m.link && m.far);
  for (const m of both) {
    setFarThere(m.far!.region, m.far!.key, null);
    delete m.far;
  }
  if (both.length) save(list);
  return list;
}

function save(list: Stored[]) {
  saveJson(KEY(), list.length ? list : null);
}

const PREFIX = "elements.perm.";

/** This device's permanent sources in the other regions, by region slug. */
function elsewhere(): Map<string, Stored[]> {
  const out = new Map<string, Stored[]>();
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      const slug = k?.startsWith(PREFIX) ? k.slice(PREFIX.length) : "";
      if (!slug || slug === region) continue;
      const v = loadJson<Stored | Stored[]>(k as StoreKey);
      const list = (Array.isArray(v) ? v : v ? [v] : []).filter(valid);
      if (list.length) out.set(slug, list);
    }
  } catch {
    // Storage blocked: no far links.
  }
  return out;
}

/** Set (or with `to` null clear) the far end on a source of region `slug`'s list. */
function setFarThere(slug: string, ticket: string, to: FarEnd | null) {
  const key = `${PREFIX}${slug}` as StoreKey;
  const list = (loadJson<Stored[]>(key) ?? []).filter(valid);
  const m = list.find((x) => x.ticket === ticket);
  if (!m) return;
  if (to) m.far = to;
  else delete m.far;
  saveJson(key, list);
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
  linked?: boolean;
};

export type PermMsg = Extract<ServerMsg, { t: "keep" | "kept" | "released" | "motto" | "link" | "gift" }>;

/** An own linked permanent source in this city, with where its partner is. */
export type Link = {
  ticket: string;
  node: number;
  ll: [number, number];
  partnerNode: number;
  partner: [number, number];
  /** The partner is in another region (`claim`/`peek` with `far`). */
  far?: { region: string; key: string; name?: string };
};

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
  /** The map: own links are drawn as lines (only this device knows the pairs). */
  map: maplibregl.Map,
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
  /** Linked permanent sources are on (`[keep] link`). */
  let linkOn = false;
  /** A link (or unlink, `b` null) sent, until the server answers. */
  let pendingLink: { a: Stored; b: Stored | null } | null = null;
  /** Tag lines just chosen, until the source list shows them. */
  const chosen = new Map<number, string>();
  /** Own sources' tag lines as last seen on the map (kept when out of view). */
  const shownMotto = new Map<number, string>();

  const name = (k: string) => t(`perm.${k}.${faction ?? "fire"}` as Key);

  // Own links as dashed lines between the two sources.
  map.addSource("links", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({
    id: "links",
    type: "line",
    source: "links",
    layout: { "line-cap": "round" },
    paint: { "line-color": ["get", "color"], "line-width": 2.5, "line-dasharray": [1.5, 2], "line-opacity": 0.85 },
  });
  let linksKey = "";
  function drawLinks() {
    const pairs = links().filter((l) => l.ticket < (mine.find((m) => m.node === l.partnerNode)?.ticket ?? ""));
    const key = pairs.map((l) => `${l.node}-${l.partnerNode}`).join(",") + (faction ?? "");
    if (key === linksKey) return;
    linksKey = key;
    (map.getSource("links") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: pairs.map((l) => ({
        type: "Feature",
        properties: { color: COLORS[faction ?? "fire"] },
        geometry: { type: "LineString", coordinates: [l.ll, l.partner] },
      })),
    });
  }

  /** Own linked sources with a known position for both ends (claim.ts, the map). */
  function links(): Link[] {
    if (!linkOn) return [];
    const out: Link[] = [];
    for (const m of mine) {
      const a = where(m);
      if (m.far && a) {
        const { region: r, key, name, ll } = m.far;
        out.push({ ticket: m.ticket, node: m.node, ll: a, partnerNode: -1, partner: ll ?? a, far: { region: r, key, name } });
        continue;
      }
      const p = m.link ? mine.find((x) => x.ticket === m.link) : undefined;
      const b = p && where(p);
      if (p && a && b) out.push({ ticket: m.ticket, node: m.node, ll: a, partnerNode: p.node, partner: b });
    }
    return out;
  }

  /** "Verbinden …" or "Verbindung lösen" for an own source (list, junction card). */
  function linkButton(m: Stored): HTMLButtonElement {
    const lb = document.createElement("button");
    lb.className = "ghost link";
    lb.textContent = m.link || m.far ? t("perm.unlink") : t("perm.link");
    lb.onclick = () => {
      if (m.far) {
        // Far links live on this device only: both ends here.
        setFarThere(m.far.region, m.far.key, null);
        delete m.far;
        save(mine);
        toast(t("perm.link.off"), 6000);
      } else if (m.link) {
        pendingLink = { a: m, b: null };
        net.send({ t: "link", node: m.node, ticket: m.ticket, to: null });
      } else pickLink(m);
    };
    return lb;
  }
  /** Where `m`'s link goes, as a button that goes there (another city:
   *  opens it there); `leave` closes what it sits in first. */
  function partnerButton(m: Stored, leave: () => void): HTMLButtonElement | null {
    const p = partner(m);
    if (!p) return null;
    const pb = document.createElement("button");
    pb.className = "ghost link partner";
    pb.textContent = `↔ ${partnerName(p)}`;
    pb.onclick = () => {
      if ("here" in p) {
        leave();
        return focus(p.here.node);
      }
      void listRegions().then((rs) => {
        const there = rs.find((x) => x.slug === p.region);
        if (there) void openRegion(there, false, p.ll);
      });
    };
    return pb;
  }
  /** "Schlüssel verschenken": a key for someone else (server `gift`). */
  function giftButton(m: Stored): HTMLButtonElement {
    const gb = document.createElement("button");
    gb.className = "ghost link";
    gb.textContent = t("gift.button");
    gb.onclick = () => net.send({ t: "gift", node: m.node, ticket: m.ticket });
    return gb;
  }
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

  /** An own source's name here ("Glut 2"): by when it was placed, not by rank. */
  const label = (m: Stored) => `${name("one")} ${mine.indexOf(m) + 1}`;
  /** Where `m`'s link goes: an own source here, or one in another city (its name and number there). */
  function partner(m: Stored): { here: Stored } | { region: string; city: string; n: number; ll?: [number, number] } | null {
    if (m.link) {
      const p = mine.find((x) => x.ticket === m.link);
      return p ? { here: p } : null;
    }
    if (!m.far) return null;
    const there = elsewhere().get(m.far.region) ?? [];
    const i = there.findIndex((x) => x.ticket === m.far!.key);
    return { region: m.far.region, city: m.far.name ?? m.far.region, n: i + 1, ll: m.far.ll };
  }
  const partnerName = (p: NonNullable<ReturnType<typeof partner>>) =>
    "here" in p ? label(p.here) : p.n > 0 ? `${p.city} · ${name("one")} ${p.n}` : p.city;

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

  // Which one to link with: a card listing the others.
  function pickLink(a: Stored) {
    const box = $("perm-pick");
    $("perm-pick-title").textContent = name("link.pick");
    const list = $("perm-pick-list");
    const others = ranked().filter((r) => r.m.ticket !== a.ticket);
    // Own sources in the other regions of this server.
    const far = elsewhere();
    const got = gifts().filter((g) => g.region !== region);
    if (far.size || got.length)
      void listRegions().then((rs) => {
        const hereName = rs.find((r) => r.slug === region)?.name;
        // Keys given by others.
        for (const g of got) {
          const r = rs.find((x) => x.slug === g.region && !x.url);
          if (!r) continue;
          const btn = document.createElement("button");
          btn.textContent = `${r.name} · ${t(g.origin === "ember" ? "found.label.ember" : g.origin === "bottle" ? "found.label.bottle" : g.origin === "city" ? "city.label" : "gift.label")}`;
          btn.onclick = () => {
            box.hidden = true;
            if (a.far) setFarThere(a.far.region, a.far.key, null);
            a.far = { region: r.slug, key: g.key, name: r.name };
            save(mine);
            toast(t("perm.link.set"), 6000);
          };
          list.append(btn);
        }
        for (const r of rs) {
          const there = far.get(r.slug);
          if (!there || r.url) continue;
          there.forEach((b, i) => {
            const btn = document.createElement("button");
            btn.textContent = `${r.name} · ${name("one")} ${i + 1}`;
            // Linked to another city: picking it moves that link here.
            if (b.far && !b.link) btn.textContent += ` · ${t("perm.linked.to", { city: b.far.name ?? b.far.region })}`;
            else if (b.link) btn.textContent += ` · ${t("perm.linked")}`;
            // Linked within its own city: only that city's server can undo it.
            if (b.link) {
              btn.disabled = true;
              btn.textContent += ` (${t("perm.linked.there", { city: r.name })})`;
              list.append(btn);
              return;
            }
            btn.onclick = () => {
              box.hidden = true;
              linkFar(a, { slug: r.slug, name: r.name }, b, hereName);
            };
            list.append(btn);
          });
        }
      });
    list.replaceChildren(
      ...others.map((r) => {
        const b = document.createElement("button");
        // Linked already (here or to another city): linking moves that link.
        const p = partner(r.m);
        b.textContent = label(r.m) + (line(r) ? ` · ${line(r)}` : "") + (p ? ` · ${t("perm.linked.to", { city: partnerName(p) })}` : "");
        b.onclick = () => {
          box.hidden = true;
          pendingLink = { a, b: r.m };
          net.send({ t: "link", node: a.node, ticket: a.ticket, to: r.m.node, to_ticket: r.m.ticket });
        };
        return b;
      }),
    );
    $("perm-pick-no").onclick = () => (box.hidden = true);
    box.hidden = false;
  }

  /** Link `a` here with `b` of region `slug` (this device only; the server checks at each claim). */
  function linkFar(a: Stored, { slug, name: regionName }: { slug: string; name: string }, b: Stored, hereName?: string) {
    if (a.far) setFarThere(a.far.region, a.far.key, null);
    if (b.far) {
      const old = b.far;
      if (old.region === region) {
        const x = mine.find((m) => m.ticket === old.key);
        if (x) delete x.far;
      } else setFarThere(old.region, old.key, null);
    }
    a.far = { region: slug, key: b.ticket, ll: b.ll, name: regionName };
    save(mine);
    setFarThere(slug, b.ticket, { region, key: a.ticket, ll: where(a), name: hereName });
    toast(t("perm.link.set"), 6000);
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
      ...ranked().map((r) => {
        const li = document.createElement("li");
        const b = document.createElement("button");
        b.textContent = label(r.m) + (line(r) ? ` · ${line(r)}` : "");
        b.onclick = () => {
          ($("board-close") as HTMLButtonElement).click();
          focus(r.m.node);
        };
        li.append(b);
        const pb = linkOn ? partnerButton(r.m, () => ($("board-close") as HTMLButtonElement).click()) : null;
        if (pb) li.append(" ", pb);
        // Linked permanent sources: link with another own one, or unlink.
        if (linkOn && (mine.length >= 2 || elsewhere().size || gifts().length || r.m.link || r.m.far)) li.append(" ", linkButton(r.m));
        if (linkOn) li.append(" ", giftButton(r.m));
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
    const gone = mine.find((m) => m.ticket === ticket);
    if (gone) shownMotto.delete(gone.node);
    mine = mine.filter((m) => m.ticket !== ticket);
    // Its partner is on its own again.
    for (const m of mine) if (m.link === ticket) delete m.link;
    if (gone?.far) setFarThere(gone.far.region, gone.far.key, null);
    save(mine);
    drawLinks();
  }

  // A key that came with this visit.
  const came = arrivedGift();
  if (came)
    void listRegions().then((rs) => {
      const r = rs.find((x) => x.slug === came.region);
      toast(t(r && !r.url ? (came.origin === "city" ? "city.got" : "gift.got") : "gift.elsewhere", { region: r?.name ?? came.region }), 9000);
    });

  return {
    /** The nodes of this device's permanent sources here. */
    nodes,
    links,
    /** For the junction card of an own source: where its link goes, and link / unlink. */
    linkControl(node: number, leave: () => void): HTMLElement | null {
      const m = mine.find((x) => x.node === node);
      // Something to link with, or a link to show (and undo).
      if (!m || !linkOn || (mine.length < 2 && !elsewhere().size && !gifts().length && !m.link && !m.far)) return null;
      const box = document.createElement("span");
      box.className = "link-ctl";
      const pb = partnerButton(m, leave);
      box.append(...(pb ? [pb, " "] : []), linkButton(m));
      return box;
    },
    drawList,
    onWelcome(
      m: { keep?: boolean; keep_used?: number; keep_base?: number; keep_per_level?: number; keep_gap_m?: number; keep_link?: boolean; mottos?: MottoItem[]; levels?: boolean },
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
      linkOn = !!m.keep_link;
      faction = f;
      drawLinks();
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
          // A move takes the tag line along (the server keeps it on the source).
          const from = moved ? mine.find((x) => x.ticket === moved)?.node : undefined;
          const carried = from === undefined ? "" : (shownMotto.get(from) ?? "");
          if (moved) forget(moved);
          else used++;
          const placed = { ticket: m.ticket, node: m.node, ...(spot ? { ll: spot.ll } : {}) };
          mine.push(placed);
          save(mine);
          if (carried) shownMotto.set(m.node, carried);
          toast(name(moved ? "moved" : "placed"), 6000);
          focus(m.node);
          if (!carried) offerMotto(placed);
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
      } else if (m.t === "gift") {
        if (m.ok && m.key) showGift(region, m.key);
        else toast(t(m.reason === "soon" ? "gift.soon" : "perm.failed"));
      } else if (m.t === "link") {
        const p = pendingLink;
        pendingLink = null;
        if (m.ok && p) {
          // A far link of either end goes (one link per source).
          const moved = [p.a, p.b].find((x) => x?.far)?.far?.name;
          for (const x of [p.a, p.b]) {
            if (x?.far) {
              setFarThere(x.far.region, x.far.key, null);
              delete x.far;
            }
          }
          if (p.b) {
            // Old partners of either end are on their own again.
            for (const x of mine) if (x.link === p.a.ticket || x.link === p.b.ticket) delete x.link;
            p.a.link = p.b.ticket;
            p.b.link = p.a.ticket;
          } else {
            for (const x of mine) if (x.link === p.a.ticket) delete x.link;
            delete p.a.link;
          }
          save(mine);
          drawLinks();
          toast(p.b && moved ? t("perm.link.moved", { city: moved }) : t(p.b ? "perm.link.set" : "perm.link.off"), 6000);
        } else toast(t("perm.failed"));
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
      drawLinks();
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
