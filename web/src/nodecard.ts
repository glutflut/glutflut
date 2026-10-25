// Card of a tapped junction (server: info.rs): who holds it and how strongly,
// its source, whether you can take it (or when), the terrain and the
// surroundings. Tapping opens it (in sim mode: right click / long press,
// since a tap moves there); "Hier erobern" claims from the card. While open,
// its timers count down, and it asks again when one runs out, the junction
// changes or the player moves.
import * as maplibregl from "maplibre-gl";
import { t, type Key } from "./i18n";
import type { Faction, Net, ServerMsg } from "./net";
import { distM } from "./geo";
import { $ } from "./page";

export type InfoMsg = Extract<ServerMsg, { t: "info" }>;

function span(s: number): string {
  // 59.5 s and up is a minute (never "60 s").
  if (s < 59.5) return t("node.t.s", { n: Math.max(1, Math.round(s)) });
  if (s < 3600) return t("node.t.min", { n: Math.round(s / 60) });
  return t("node.t.h", { n: Math.round(s / 360) / 10 });
}

export function setupNodeCard(
  map: maplibregl.Map,
  net: Net,
  claim: (node: number) => void,
  faction: () => Faction | null,
  ownKeep: () => ReadonlySet<number>,
  owner: (f: number, w: number) => number,
  /** The claim button's wait (cooldown or moving), if any. */
  claimWait: () => { s: number; moving: boolean } | null,
  me: () => [number, number] | null,
  /** The link button for an own permanent source (perm.ts), if any. */
  linkControl: (node: number) => HTMLElement | null,
) {
  const card = $("node-card");
  let node: number | null = null;
  let marker: maplibregl.Marker | null = null;
  /** The last answer and when it came. */
  let last: InfoMsg | null = null;
  let got = 0;
  /** The link button shown, as node and label (rebuilt only when that changes). */
  let linkShown = "";

  // Ask again, at most once a second.
  let asked = 0;
  let later = 0;
  function ask() {
    if (node === null) return;
    const d = 1000 - (Date.now() - asked);
    if (d > 0) {
      later ||= window.setTimeout(() => {
        later = 0;
        ask();
      }, d);
      return;
    }
    asked = Date.now();
    net.send({ t: "info", node });
  }

  function close() {
    card.hidden = true;
    node = null;
    last = null;
    marker?.remove();
    marker = null;
  }

  function open(id: number, at: [number, number]) {
    node = id;
    last = null;
    marker?.remove();
    marker = new maplibregl.Marker({ element: Object.assign(document.createElement("div"), { className: "node-pick" }) })
      .setLngLat(at)
      .addTo(map);
    asked = 0;
    ask();
  }

  $("node-close").onclick = close;
  $("node-claim").onclick = () => node !== null && claim(node);

  function show(m: InfoMsg) {
    if (m.node !== node) return;
    last = m;
    got = Date.now();
    render(m);
  }

  // Timers count down between answers; one running out asks again.
  const less = (v: number | undefined, el: number) => (v === undefined ? v : Math.max(0, v - el));
  setInterval(() => {
    if (!last || card.hidden) return;
    const el = (Date.now() - got) / 1000;
    const s = last.source;
    const due = [last.wait, s?.left, s?.build, s?.kind === "keep" ? s.gone : undefined].some((v) => v && v - el <= 0);
    // The server takes a new position on its next tick: ask until it agrees.
    const p = me();
    const at = marker?.getLngLat();
    const moved = p && at && Math.abs(distM(p, [at.lng, at.lat]) - last.dist) > 10;
    if (due || moved) ask();
    render({
      ...last,
      wait: less(last.wait, el),
      source: s && { ...s, left: less(s.left, el), build: less(s.build, el)!, gone: less(s.gone, el) },
    });
  }, 1000);

  function render(m: InfoMsg) {
    const me = faction();
    const o = owner(m.fire, m.water);
    const side = o === 1 ? "fire" : o === 2 ? "water" : o === 3 ? "front" : "neutral";
    card.className = o === 1 ? "fire" : o === 2 ? "water" : "";
    $("node-kicker").textContent = t(`node.${side}` as Key);
    ($("node-fire") as HTMLElement).style.width = `${Math.round(m.fire * 100)}%`;
    ($("node-water") as HTMLElement).style.width = `${Math.round(m.water * 100)}%`;
    $("node-values").textContent = t("node.values", { f: Math.round(m.fire * 100), w: Math.round(m.water * 100) });

    // Source.
    const s = m.source;
    let src = t("node.src.none");
    if (s) {
      const sideName = t(`side.${s.faction}` as Key);
      const left = s.left !== undefined ? span(s.left) : "";
      const mine = s.yours || (s.kind === "keep" && ownKeep().has(m.node));
      src =
        s.kind === "natural"
          ? t("node.src.natural", { side: sideName })
          : s.kind === "spark"
            ? t(`node.src.spark.${s.faction}` as Key, { t: left })
            : s.kind === "keep"
              ? s.left === 0
                ? // At its floor: stays until it expires without a top-up.
                  t(`node.src.keep.${s.faction}.faded${mine ? ".yours" : ""}` as Key, { t: span(s.gone ?? 0) })
                : t(`node.src.keep.${s.faction}${mine ? ".yours" : ""}` as Key, { t: left })
              : t(`node.src.${s.kind}${mine ? ".yours" : ""}` as Key, { side: sideName, t: left });
      if (s.build > 0) src += t("node.build", { t: span(s.build) });
    }
    $("node-source").textContent = src;
    // Own permanent source: link it from here (or unlink).
    const slot = $("node-link");
    const own = s?.kind === "keep" && ownKeep().has(m.node);
    const lb = own ? linkControl(m.node) : null;
    const key = lb ? `${m.node}:${lb.textContent}` : "";
    if (key !== linkShown) {
      linkShown = key;
      slot.replaceChildren(...(lb ? [lb] : []));
      slot.hidden = !lb;
    }

    // Can I take it?
    const take =
      m.take === "strong"
        ? m.wait
          ? t("node.take.strong", { t: span(m.wait) })
          : t("node.take.strong.long")
        : m.take === "range"
          ? t("node.take.range", { m: m.dist, r: m.reach })
          : m.take === "anchor"
            ? t(me === "fire" ? "node.take.anchor.fire" : "node.take.anchor.water")
            : t(`node.take.${m.take}` as Key);
    $("node-take").textContent = take;
    $("node-take").className = m.take === "ok" || m.take === "anchor" ? "go" : "";

    // Terrain: what the height does for your element here.
    const good = me && ((m.terrain === "peak" && me === "fire") || (m.terrain === "sink" && me === "water"));
    const bad = me && ((m.terrain === "peak" && me === "water") || (m.terrain === "sink" && me === "fire"));
    $("node-terrain").textContent =
      t(`node.terrain.${m.terrain}` as Key, { elev: Math.round(m.elev), dh: Math.abs(m.dh).toLocaleString(undefined, { maximumFractionDigits: 1 }) }) +
      (good ? t("node.terrain.good") : bad ? t("node.terrain.bad") : "");
    $("node-lean").textContent = m.lean ? t(`node.lean.${m.lean}` as Key) : "";
    $("node-lean").hidden = !m.lean;
    // Rain radar (when there is data): rain here now, and how old the picture is.
    const rain = $("node-rain");
    rain.hidden = m.rain === undefined || !m.rain_at;
    if (!rain.hidden) {
      const time = new Date(m.rain_at!).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      rain.textContent =
        m.rain! > 0
          ? t("node.rain", { mm: m.rain!.toLocaleString(undefined, { maximumFractionDigits: 1 }), time })
          : t("node.rain.dry", { time });
    }

    const btn = $("node-claim") as HTMLButtonElement;
    btn.hidden = !(m.take === "ok" || m.take === "anchor");
    btn.className = me ?? "";
    const w = claimWait();
    btn.disabled = !!w;
    btn.textContent = w
      ? t(w.moving ? "claim.wait.moving" : "claim.wait", { s: w.s })
      : m.take === "anchor"
        ? t(me === "fire" ? "claim.anchor.go.fire" : "claim.anchor.go.water")
        : t("node.claim");
    card.hidden = false;
  }

  return {
    open,
    close,
    onInfo: show,
    /** Ask again for the open card (reach, side, level, position changed). */
    refresh: ask,
    /** These junctions changed: ask again if the open one is among them. */
    changed(ids: Iterable<number>) {
      if (node === null) return;
      for (const id of ids) if (id === node) return ask();
    },
    /** A claim answer: refresh the card if it was this junction. */
    onClaim(id: number) {
      if (id === node) setTimeout(() => node === id && ask(), 400);
    },
  };
}
