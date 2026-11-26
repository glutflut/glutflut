// Carrying (server `carry.rs`): take a load at an own source (jug for water,
// lantern for fire), carry it to your side's site or put it down for someone
// else; enough loads at the site make a building with the builders' names.
// Spilling on the way (spill.ts) counts at delivery. The other side sees
// only the buildings.
import * as maplibregl from "maplibre-gl";
import { distM } from "./geo";
import { t, type Key } from "./i18n";
import type { Faction, Net } from "./net";
import { toast } from "./page";
import type { BuiltView, CarryView, LoadsView } from "./protocol";
import { Feel, askMotion, feelView, type FeelState } from "./spill";
import type { Sound } from "./sound";
import { load as stored, save, KEYS } from "./store";

/** Tap a lying load from at most this far (m; the server allows 30 around the junction). */
const NEAR_M = 40;

export function setupCarry(
  map: maplibregl.Map,
  net: Net,
  sound: Sound,
  me: () => [number, number] | null,
  faction: () => Faction | null,
  notice: (cls: string, kicker: string, name: string, line: string) => void,
) {
  // The panel above the buttons while carrying.
  const panel = document.createElement("div");
  panel.id = "carry";
  panel.hidden = true;
  panel.innerHTML = `<div class="carry-view"></div><div class="carry-info"><b class="carry-what"></b><span class="carry-fill"></span><span class="carry-left"></span></div><button class="carry-lay"></button>`;
  const row = document.querySelector("#actions .row");
  row?.parentElement?.insertBefore(panel, row);
  const q = <T extends HTMLElement>(s: string) => panel.querySelector<T>(s)!;

  let held: CarryView | null = null;
  let heldAt = 0;
  let feel: Feel | null = null;
  let draw: ((s: FeelState) => void) | null = null;
  let lastSound = 0;
  let lying: maplibregl.Marker[] = [];
  let site: maplibregl.Marker | null = null;
  let builds: maplibregl.Marker[] = [];
  let loads: LoadsView = { lying: [], site: null };
  /** The server plays carrying (it said so on connect). */
  let on = false;

  const kind = (f: Faction) => t(f === "water" ? "carry.jug" : "carry.lantern");
  const building = (f: Faction) => t(f === "water" ? "carry.well" : "carry.forge");
  /** A sentence per side (articles differ: einen Krug, eine Laterne). */
  const say = (key: string, f: Faction, vars?: Record<string, string | number>) => t(`${key}.${f}` as Key, vars);
  const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
  const num = (v: number) => v.toLocaleString(undefined, { maximumFractionDigits: 1 });

  function take() {
    // iOS asks for the motion sensors only after a tap: this is one.
    void askMotion();
    net.send({ t: "take" });
  }
  q<HTMLButtonElement>(".carry-lay").onclick = () => net.send({ t: "lay", fill: feel?.fill ?? held?.fill ?? 1 });

  function setHeld(v: CarryView | null) {
    const was = held;
    held = v;
    heldAt = performance.now();
    if (!v) {
      feel?.stop();
      feel = null;
      draw = null;
      panel.hidden = true;
      return;
    }
    // A new load (not just an update of the one carried): a new feel.
    if (!was || !feel || feel.kind !== v.side) {
      feel?.stop();
      feel = new Feel(v.side, (lost) => {
        const now = Date.now();
        if (now - lastSound < 700) return;
        lastSound = now;
        if (v.side === "water") sound.slosh(lost * 20);
        else sound.tumble(lost * 20);
      });
      feel.start();
      const view = feelView(v.side);
      q(".carry-view").replaceChildren(view.el);
      draw = view.draw;
      feel.fill = v.fill;
    }
    feel.fill = Math.min(feel.fill, v.fill);
    panel.className = v.side;
    q(".carry-what").textContent = kind(v.side);
    q(".carry-lay").textContent = t(loads.site && atSite() ? "carry.deliver" : "carry.lay");
    panel.hidden = false;
  }

  function atSite(): boolean {
    const p = me();
    return !!(p && loads.site && distM(p, [loads.site.lon, loads.site.lat]) <= 40);
  }

  let t0 = performance.now();
  function frame(now: number) {
    const dt = Math.min(0.1, (now - t0) / 1000);
    t0 = now;
    if (held && feel && draw) {
      const st = feel.tick(dt);
      draw(st);
      q(".carry-fill").textContent = `${Math.round(st.fill * 100)} %`;
      const left = Math.max(0, held.left - (now - heldAt) / 1000);
      q(".carry-left").textContent = mmss(left);
      q(".carry-left").classList.toggle("low", left < 60);
      q(".carry-lay").textContent = t(atSite() ? "carry.deliver" : "carry.lay");
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);

  function marker(cls: string, html: string, at: [number, number], onTap: () => void): maplibregl.Marker {
    const el = document.createElement("div");
    el.className = cls;
    el.innerHTML = html;
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      onTap();
    });
    return new maplibregl.Marker({ element: el }).setLngLat(at).addTo(map);
  }

  function drawLoads() {
    for (const m of lying) m.remove();
    site?.remove();
    site = null;
    const side = faction();
    if (!side) return;
    lying = loads.lying.map((l) =>
      marker(`carry-lying ${side}`, side === "water" ? "🏺" : "🏮", [l.lon, l.lat], () => {
        const p = me();
        const d = p ? distM(p, [l.lon, l.lat]) : Infinity;
        if (!held && d <= NEAR_M) return take();
        toast(say("carry.lying.hint", side, { h: Math.max(1, Math.round(l.left / 3600)), n: l.hands }), 5000);
      }),
    );
    const s = loads.site;
    if (s) {
      const pct = Math.round((s.filled / s.need) * 100);
      site = marker(
        `carry-site ${side}`,
        `<div class="carry-site-ring" style="--p:${pct}%"></div><span>${num(s.filled)}/${num(s.need)}</span>`,
        [s.lon, s.lat],
        () => toast(say("carry.site.hint", side, { n: num(s.filled), of: num(s.need), d: Math.max(1, Math.round(s.left / 86400)) }), 6000),
      );
    }
  }

  return {
    /** The junction card's button: take at an own source. */
    take,
    carrying: () => held !== null,
    on: () => on,
    onCarry(m: { load: CarryView | null }) {
      on = true;
      setHeld(m.load);
    },
    onDone(m: { what: string; filled?: number; need: number }) {
      const side = held?.side ?? faction() ?? "water";
      if (m.what === "fresh") toast(say("carry.fresh", side), 6000);
      else if (m.what === "handed") toast(say("carry.handed", side), 4000);
      else if (m.what === "waiting") toast(say("carry.waiting", side), 5000);
      else if (m.what === "delivered") toast(t("carry.delivered", { n: num(m.filled ?? 0), of: num(m.need) }), 4000);
      else if (m.what === "tired") toast(say("carry.tired", side), 6000);
      if (m.what === "fresh" || m.what === "handed") sound.chime();
    },
    onFail(m: { reason: string }) {
      const key = ({ "nothing here": "carry.fail.here", carrying: "carry.fail.carrying", "no street here": "carry.fail.street", "no position": "carry.fail.pos" } as Record<string, Key>)[m.reason];
      if (key) toast(t(key), 4000);
    },
    onLoads(m: LoadsView) {
      loads = m;
      drawLoads();
      // The first time there is a site: what this is about.
      if (m.site && stored(KEYS.carrySeen) !== "1") {
        save(KEYS.carrySeen, "1");
        const side = faction() ?? "water";
        toast(say("carry.intro", side), 8000);
      }
    },
    onBuilds(m: { items: BuiltView[] }) {
      for (const b of builds) b.remove();
      builds = m.items.map((b) =>
        marker(`carry-built ${b.side}`, b.side === "water" ? "⛲" : "⚒️", [b.lon, b.lat], () => {
          const when = new Date(b.when * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
          toast(say("carry.built.hint", b.side, { when, names: b.names.join(", ") || "…" }), 6000);
        }),
      );
    },
    onBuilt(b: BuiltView) {
      notice(b.side, t("carry.built.kicker"), building(b.side), t("carry.built.line", { names: b.names.join(", ") || "…" }));
      sound.motif();
      navigator.vibrate?.([60, 40, 60, 40, 200]);
    },
  };
}
