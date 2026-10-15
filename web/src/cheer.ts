// Cheering (server cheer.rs): watchers tap 🔥 or 💧, players their own side;
// everyone sees how many cheer for each side in the last minutes, and new
// cheers rise as small flames and drops on visible streets. For show only: no points, no effect.
import * as maplibregl from "maplibre-gl";
import { t } from "./i18n";
import { COLORS } from "./geo";
import { $ } from "./page";
import type { ClientMsg, Faction, ServerMsg } from "./protocol";

/** The server takes one cheer per connection in this time. */
const GAP_MS = 3_000;
/** At most this many icons on the map at once, cheers shown per side per
 * message, and icons per cheer (a small cluster). */
const ICONS_MAX = 36;
const PER_BURST = 5;
const PER_CHEER = 3;
/** Cheers a hidden page got show when it is back within this time. */
const MISSED_MS = 15_000;

const SHAPES: Record<Faction, string> = {
  fire: "M9 2C12 7 16 9 15 14C14.5 18.5 11.5 20 9 20C6.5 20 3.5 18.5 3 14C2.5 10.5 5.5 9 6.5 5.5C8 8 9.5 6 9 2Z",
  water: "M9 2C11 7 15 10 15 14A6 6 0 0 1 3 14C3 10 7 7 9 2Z",
};

function iconEl(side: Faction, delayMs: number): HTMLElement {
  const el = document.createElement("div");
  el.className = "float-at";
  const s = el.appendChild(document.createElement("span"));
  s.className = `float-cheer ${side}`;
  s.innerHTML = `<svg viewBox="0 0 18 22" width="28" height="34"><path d="${SHAPES[side]}" fill="${COLORS[side]}" stroke="#fff" stroke-width="1.6" stroke-linejoin="round"/></svg>`;
  s.style.animationDelay = `${Math.round(delayMs)}ms`;
  return el;
}

export function setupCheer(
  map: maplibregl.Map,
  nodePos: Map<number, [number, number]>,
  send: (m: ClientMsg) => boolean,
  buttons: boolean,
) {
  let icons = 0;

  /** A street junction in the middle of the screen (away from the edges and
   * the buttons), else any point there. */
  function spots(n: number): [number, number][] {
    const c = map.getContainer();
    const [w, h] = [c.clientWidth, c.clientHeight];
    const box = new maplibregl.LngLatBounds(map.unproject([w * 0.2, h * 0.7]), map.unproject([w * 0.8, h * 0.25]));
    const seen: [number, number][] = [];
    for (const ll of nodePos.values()) if (box.contains(ll)) seen.push(ll);
    return Array.from({ length: n }, () => {
      if (seen.length) return seen[Math.floor(Math.random() * seen.length)];
      const ll = map.unproject([w * (0.2 + Math.random() * 0.6), h * (0.25 + Math.random() * 0.45)]);
      return [ll.lng, ll.lat];
    });
  }

  function rise(side: Faction, n: number) {
    n = Math.min(n, PER_BURST, Math.floor((ICONS_MAX - icons) / PER_CHEER));
    if (n <= 0) return;
    for (const ll of spots(n)) {
      // Not all at once: a burst trickles up, each cheer as a small cluster.
      const start = Math.random() * 1500;
      for (let k = 0; k < PER_CHEER; k++) {
        icons++;
        const el = iconEl(side, start + k * 280);
        const offset: [number, number] = [(k - 1) * 18 + (Math.random() - 0.5) * 8, k === 1 ? -10 : 0];
        const m = new maplibregl.Marker({
          element: el,
          anchor: "bottom",
          offset,
        })
          .setLngLat(ll)
          .addTo(map);
        el.firstElementChild!.addEventListener(
          "animationend",
          () => {
            m.remove();
            icons--;
          },
          { once: true },
        );
      }
    }
  }

  const line = $("cheers");
  function show(fire: number, water: number) {
    line.hidden = fire + water === 0;
    if (line.hidden) return;
    line.replaceChildren();
    const part = (side: Faction, n: number, icon: string) => {
      const s = line.appendChild(document.createElement("span"));
      s.className = side;
      s.textContent = `${icon} ${n}`;
    };
    part("fire", fire, "🔥");
    line.append(" · ");
    part("water", water, "💧");
    line.append(` ${t("cheer.line")}`);
  }

  if (buttons) {
    for (const side of ["fire", "water"] as const) {
      const b = $(`cheer-${side}`) as HTMLButtonElement;
      const label = t("cheer.for", { side: t(`side.${side}`) });
      b.title = label;
      b.setAttribute("aria-label", label);
      b.onclick = () => {
        if (!send({ t: "cheer", side })) return;
        b.classList.remove("pop");
        void b.offsetWidth;
        b.classList.add("pop");
        // Like the server: one cheer per few seconds.
        for (const o of ["fire", "water"]) ($(`cheer-${o}`) as HTMLButtonElement).disabled = true;
        setTimeout(() => {
          for (const o of ["fire", "water"]) ($(`cheer-${o}`) as HTMLButtonElement).disabled = false;
        }, GAP_MS);
      };
    }
  }

  // A hidden page (other tab, screen off) keeps fresh cheers for when it shows.
  let missed = { fire: 0, water: 0, at: 0 };
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) return;
    if (Date.now() - missed.at < MISSED_MS) {
      rise("fire", missed.fire);
      rise("water", missed.water);
    }
    missed = { fire: 0, water: 0, at: 0 };
  });

  return {
    onMsg(m: Extract<ServerMsg, { t: "cheers" }>) {
      show(m.fire, m.water);
      if (document.hidden) {
        missed = {
          fire: missed.fire + m.burst[0],
          water: missed.water + m.burst[1],
          at: Date.now(),
        };
        return;
      }
      rise("fire", m.burst[0]);
      rise("water", m.burst[1]);
    },
  };
}
