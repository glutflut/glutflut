// Watching before playing: a first visit sees the live game, but is no player
// until it taps "Mitspielen" (no token, no row on the server, nothing counted;
// the server passes on only view, overview and node info). Players with a
// token play at once; test servers (`?sim`) and `?play` too.
import { t } from "./i18n";
import { $ } from "./page";
import { locate, regionAt, type RegionInfo } from "./region";
import { KEYS, load, save } from "./store";

const params = new URLSearchParams(location.search);

/** A screen (`?kiosk`): always only watching, no buttons, a camera tour (live.ts). */
export const kiosk = params.has("kiosk");

/** Players who picked another city (city list, globe) watch there: `?watch`. */
export const watching = (): boolean =>
  kiosk ||
  params.has("watch") ||
  (!load(KEYS.token) && load(KEYS.joined) !== "1" && !params.has("sim") && !params.has("play"));

let gate: { slug: string; regions: () => RegionInfo[] } | null = null;
let pending = false;

type JoinKey = "watch.join" | "watch.locate" | "watch.locating" | "watch.away" | "watch.nopos";
function show(key: JoinKey, on: boolean) {
  const btn = $("join") as HTMLButtonElement;
  btn.textContent = t(key);
  btn.disabled = !on;
}

/** Ask for the position; "Mitspielen" only with one in this city. */
export async function tryJoin() {
  if (!gate) return void (pending = true);
  const { slug, regions } = gate;
  show("watch.locating", false);
  const pos = await locate(10_000);
  if (!pos) return show("watch.nopos", true);
  if (regionAt(regions(), pos)?.slug !== slug) return show("watch.away", false);
  show("watch.join", true);
  ($("join") as HTMLButtonElement).onclick = join;
}

/** "Mitspielen" needs a position in this city: playing means being there.
 *  Checks at once if the browser already allows the position, else on a tap. */
export async function joinGate(slug: string, regions: () => RegionInfo[]) {
  gate = { slug, regions };
  ($("join") as HTMLButtonElement).onclick = () => void tryJoin();
  const perm = await navigator.permissions?.query({ name: "geolocation" }).catch(() => null);
  if (pending || perm?.state === "granted") void tryJoin();
  else show("watch.locate", true);
}

/** Join: from now on this browser plays (a new socket says hello as a player). */
export function join() {
  save(KEYS.joined, "1");
  const u = new URL(location.href);
  u.searchParams.delete("watch");
  location.href = u.toString();
}
