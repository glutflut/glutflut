// Fool's day (Fasnacht, `[fool]`): everything the other way round. The
// server turns the slopes over (fire runs downhill, water climbs) and says so
// in `welcome`; the app swaps the sides' colours, its own UI included, and
// the street animation. Colours go into the map when it is built, so a
// change reloads the app once; the day is remembered on the device.
import { COLORS, CORE, GLOW } from "./geo";
import { KEYS, load, save } from "./store";

const today = () => new Date().toLocaleDateString("sv"); // YYYY-MM-DD, local

export const FOOL = { on: false };

/** Before the map: swap if today is a known fool's day. */
export function startFool() {
  FOOL.on = load(KEYS.fool) === today();
  if (!FOOL.on) return;
  for (const c of [COLORS, GLOW, CORE] as Record<string, string>[]) [c.fire, c.water] = [c.water, c.fire];
  document.documentElement.classList.add("fool");
}

/** The server's word; true if the app reloads to change over. */
export function foolFromServer(on: boolean): boolean {
  if (on === FOOL.on) return false;
  save(KEYS.fool, on ? today() : "");
  // Once per session: a device date off by a day must not loop.
  try {
    if (sessionStorage.getItem("elements.fool.reloaded")) return false;
    sessionStorage.setItem("elements.fool.reloaded", "1");
  } catch {
    return false;
  }
  location.reload();
  return true;
}
