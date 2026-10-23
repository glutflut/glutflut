// Finds (server: crates/server/src/finds.rs): named places a claim nearby
// uncovers. Found ones show on the map and in the album (settings panel);
// the first find asks whether to collect them (opt-in, stored without times).
import * as maplibregl from "maplibre-gl";
import { badgeLabel } from "./badges";
import { lang, t, type Key } from "./i18n";
import { pickLang, showStory } from "./story";
import type { FindItem, Net } from "./net";
import { confirmAdult, onAdult } from "./consent";
import { $ } from "./page";
import { KEYS, load, save } from "./store";
import { ask as question } from "./ask";
import { figure, mediaFor, preloadMedia, type Media } from "./media";

const COLOR: Record<string, string> = {
  fountains: "#4dabf7",
  trees: "#51cf66",
  gates: "#ff922b",
  views: "#ffd43b",
  art: "#da77f2",
  stones: "#ced4da",
};
/** Which element a collection belongs to (the animation). */
const ELEMENT: Record<string, "fire" | "water"> = { fountains: "water", trees: "water", gates: "fire" };

const collName = (c: string) => t(`finds.c.${c}` as Key);
/** Boundary stones are named by number in OSM. */
const findName = (f: FindItem) => (f.c === "stones" && /^\d/.test(f.name) ? t("finds.stone", { name: f.name }) : f.name);

function asked(): boolean {
  return load(KEYS.findsAsked) === "1";
}

export function setupFinds(map: maplibregl.Map, net: Net, toast: (text: string) => void) {
  let collect = false;
  let totals: Record<string, number> = {};
  const known = new Map<number, FindItem>();

  map.addSource("finds", { type: "geojson", data: { type: "FeatureCollection", features: [] } });
  map.addLayer({
    id: "finds",
    type: "circle",
    source: "finds",
    paint: {
      "circle-radius": ["interpolate", ["linear"], ["zoom"], 13, 3, 17, 7],
      "circle-color": ["get", "color"],
      "circle-stroke-color": "#16181d",
      "circle-stroke-width": 2,
    },
  });
  map.addLayer({
    id: "finds-label",
    type: "symbol",
    source: "finds",
    minzoom: 16,
    layout: {
      "text-field": ["get", "name"],
      "text-font": ["Noto Sans Medium"],
      "text-size": 12,
      "text-offset": [0, 1.1],
      "text-anchor": "top",
      "text-optional": true,
    },
    paint: { "text-color": "#f1f3f5", "text-halo-color": "#16181d", "text-halo-width": 1.5 },
  });

  function redraw() {
    (map.getSource("finds") as maplibregl.GeoJSONSource).setData({
      type: "FeatureCollection",
      features: [...known.values()].map((f) => ({
        type: "Feature",
        properties: { name: findName(f), color: COLOR[f.c] ?? "#fff" },
        geometry: { type: "Point", coordinates: [f.lon, f.lat] },
      })),
    });
  }

  function album() {
    ($("collect") as HTMLInputElement).checked = collect;
    const list = $("finds-list");
    list.replaceChildren(
      ...Object.entries(totals)
        .sort((a, b) => collName(a[0]).localeCompare(collName(b[0])))
        .map(([c, of]) => {
          const mine = [...known.values()].filter((f) => f.c === c);
          const li = document.createElement("li");
          const head = document.createElement("b");
          head.textContent = `${collName(c)} ${mine.length}/${of}`;
          head.style.color = COLOR[c] ?? "";
          li.append(head);
          // Names; a find with a text (a letter) opens it again.
          mine
            .sort((a, b) => findName(a).localeCompare(findName(b)))
            .forEach((f, i) => {
              li.append(i ? ", " : " – ");
              if (!f.text) return li.append(findName(f));
              const b = document.createElement("button");
              b.className = "find-letter";
              b.textContent = findName(f);
              b.onclick = () => {
                ($("board-close") as HTMLButtonElement).click();
                letter(f, null);
              };
              li.append(b);
            });
          return li;
        }),
    );
  }

  /** Rings (and sparks for fire) at the place, gone after a moment. */
  function burst(f: FindItem) {
    const el = document.createElement("div");
    el.className = `find-burst ${ELEMENT[f.c] ?? "other"}`;
    for (let i = 0; i < 3; i++) el.append(document.createElement("span"));
    const m = new maplibregl.Marker({ element: el }).setLngLat([f.lon, f.lat]).addTo(map);
    setTimeout(() => m.remove(), 2600);
  }

  // Several at once (a fountain with a sculpture, a badge): one card after the other.
  interface Card {
    cls: string;
    kicker: string;
    name: string;
    line: string;
    /** Ask to collect (first find, not collecting). */
    ask: boolean;
    /** "For this visit only". */
    note: boolean;
    /** Its image, if the city gave it one. */
    media?: [Media["kind"], string];
  }
  const queue: Card[] = [];
  let cardTimer = 0;
  let cardNo = 0;
  /** A letter to read once the find card (with its question) is closed. */
  let after: (() => void) | null = null;
  preloadMedia();
  function hideCard() {
    clearTimeout(cardTimer);
    $("find-card").hidden = true;
    const read = after;
    after = null;
    if (read) return read();
    const next = queue.shift();
    if (next) setTimeout(() => show(next), 300);
  }
  /** A find's text (a letter, a diary page) as a story card. */
  function letter(f: FindItem, line: string | null) {
    const text = pickLang(f.text, lang);
    if (!text) return;
    showStory(
      { cls: ELEMENT[f.c] ?? "badge", kicker: t("finds.found"), title: findName(f), line: line ?? undefined, text, media: ["find", String(f.id)] },
      () => {
        const next = queue.shift();
        if (next) setTimeout(() => show(next), 300);
      },
    );
  }
  function card(f: FindItem, n: number, of: number, stored: boolean) {
    const ask = !stored && !collect && !asked();
    const line = t("finds.progress", { coll: collName(f.c), n, of });
    // A letter: read it right away; after the question to collect, if that is due.
    if (f.text && !ask && $("find-card").hidden) return letter(f, line);
    if (f.text) after = () => letter(f, line);
    show({
      cls: ELEMENT[f.c] ?? "other",
      kicker: t("finds.found"),
      name: findName(f),
      line,
      ask,
      note: !ask && !stored && !collect,
      media: ["find", String(f.id)],
    });
  }
  function show(c: Card) {
    const box = $("find-card");
    if (!box.hidden) {
      queue.push(c);
      return;
    }
    const ask = c.ask && !collect && !asked();
    box.className = `card ${c.cls}`;
    $("find-kicker").textContent = c.kicker;
    $("find-name").textContent = c.name;
    $("find-progress").textContent = c.line;
    $("find-ask").hidden = !ask;
    $("find-note").hidden = !c.note || collect;
    const slot = $("find-media");
    slot.replaceChildren();
    const shown = ++cardNo;
    if (c.media)
      void mediaFor(...c.media).then((m) => {
        if (m && shown === cardNo) slot.append(figure(m));
      });
    box.hidden = false;
    // Restart the pop-in animation.
    box.style.animation = "none";
    void box.offsetWidth;
    box.style.animation = "";
    clearTimeout(cardTimer);
    // Long enough to notice while walking; a tap closes it sooner.
    if (!ask) cardTimer = window.setTimeout(hideCard, 12_000);
  }

  const answer = async (yes: boolean) => {
    save(KEYS.findsAsked, "1");
    if (yes && (await confirmAdult())) {
      collect = true;
      net.send({ t: "collect", on: true });
    }
    hideCard();
  };
  $("find-yes").onclick = () => answer(true);
  $("find-no").onclick = () => answer(false);
  $("find-card").onclick = (e) => {
    // Credit links open; a tap elsewhere closes.
    if (!(e.target as HTMLElement).closest("button, a") && $("find-ask").hidden) hideCard();
  };
  ($("collect") as HTMLInputElement).onchange = async (e) => {
    const box = e.target as HTMLInputElement;
    const on = box.checked;
    // Stays as it was until answered.
    box.checked = !on;
    if (on ? !(await confirmAdult()) : !(await question(t("finds.off.confirm")))) return;
    box.checked = on;
    net.send({ t: "collect", on });
  };

  // No longer 16 or older (unticked): consent gone, finds deleted.
  onAdult((on) => {
    if (!on && collect) {
      collect = false;
      net.send({ t: "collect", on: false });
    }
  });

  return {
    /** A card like a find's (campaign results). */
    notice(cls: string, kicker: string, name: string, line: string, media?: [Media["kind"], string]) {
      show({ cls, kicker, name, line, ask: false, note: false, media });
    },
    onFinds(m: { collect: boolean; items: FindItem[]; totals: Record<string, number> }) {
      collect = m.collect;
      totals = m.totals;
      known.clear();
      for (const f of m.items) known.set(f.id, f);
      redraw();
      album();
      $("finds-box").hidden = Object.keys(totals).length === 0;
    },
    onFind(m: FindItem & { n: number; of: number; stored: boolean }) {
      known.set(m.id, m);
      redraw();
      album();
      burst(m);
      card(m, m.n, m.of, m.stored);
      navigator.vibrate?.([80, 60, 80, 60, 160]);
    },
    onNear(c: string) {
      toast(t(`finds.near.${c}` as Key));
    },
    onBadge(badge: string) {
      show({ cls: "badge", kicker: t("badge.new"), name: badgeLabel(badge), line: "", ask: false, note: false });
      navigator.vibrate?.([30, 40, 30, 40, 60]);
    },
  };
}
