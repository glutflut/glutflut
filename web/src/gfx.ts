// Graphics level per browser: full (tilted, 3D buildings, moving elements),
// simple (dark and glowing, flat, still), map (the light map). "auto" picks
// full or simple from device hints; a frame-rate watchdog steps an automatic
// full down to simple and remembers that.
import { KEYS, load, save } from "./store";

export type Gfx = "full" | "simple" | "map";
export type GfxChoice = Gfx | "auto";

const KEY = KEYS.gfx;
/** Set when the watchdog stepped down; auto then starts simple. */
const AUTO_KEY = KEYS.gfxAuto;

export const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;

export function gfxChoice(): GfxChoice {
  const v = load(KEY);
  if (v === "full" || v === "simple" || v === "map" || v === "auto") return v;
  // Earlier switches (game view, animation).
  if (load(KEYS.oldView) === "map") return "map";
  if (load(KEYS.oldAnim) === "off") return "simple";
  return "auto";
}

/** Device hints only; unknown values count as capable. */
export function autoGfx(): Gfx {
  if (load(AUTO_KEY) === "simple" || calm) return "simple";
  const nav = navigator as Navigator & { deviceMemory?: number; connection?: { saveData?: boolean } };
  if (nav.connection?.saveData) return "simple";
  if ((nav.deviceMemory ?? 8) <= 2) return "simple";
  if ((navigator.hardwareConcurrency ?? 8) <= 4) return "simple";
  return "full";
}

export function setGfx(c: GfxChoice) {
  save(KEY, c);
  // A manual choice resets what the watchdog learned.
  if (c !== "auto") save(AUTO_KEY, "");
}

/**
 * After the map settled, count frames for a few seconds while the animation
 * runs; below `minFps` call `slow` once and remember it for "auto".
 */
export function watchFrames(slow: () => void, minFps = 24, afterMs = 8000, forMs = 5000) {
  setTimeout(() => {
    if (document.hidden) return;
    let n = 0;
    const t0 = performance.now();
    const tick = (now: number) => {
      n++;
      if (now - t0 < forMs) return void requestAnimationFrame(tick);
      if (!document.hidden && (n * 1000) / (now - t0) < minFps) {
        save(AUTO_KEY, "simple");
        slow();
      }
    };
    requestAnimationFrame(tick);
  }, afterMs);
}
