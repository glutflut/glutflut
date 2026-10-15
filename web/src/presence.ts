// Proof of presence (server: web.rs `Presence`): a code in
// the game link (`?p=`), e.g. on a poster or at a stand, counts once the phone
// is near its place. Kept until confirmed, at most 10 minutes; tried again
// every 15 s while not close enough yet. The code leaves the address bar, so a
// shared link doesn't pass it on.
import { KEYS, loadJson, saveJson } from "./store";

const KEY = KEYS.presence;
const KEEP_MS = 10 * 60_000;
const RETRY_MS = 15_000;

type Pending = { code: string; since: number };

function load(): Pending | null {
  const p = loadJson<Pending>(KEY);
  return p && Date.now() - p.since < KEEP_MS ? p : null;
}

function save(p: Pending | null) {
  saveJson(KEY, p);
}

let pending: Pending | null = null;
let lastTry = 0;

/** Take `?p=` from the address (call once at start). */
export function takePresenceCode() {
  const u = new URL(location.href);
  const code = u.searchParams.get("p");
  if (code && /^[a-z0-9]{4,16}$/.test(code)) save({ code, since: Date.now() });
  if (u.searchParams.has("p")) {
    u.searchParams.delete("p");
    history.replaceState(history.state, "", u);
  }
  pending = load();
}

/** With a position: the code to send now, if any (at most every 15 s). */
export function presenceToSend(): string | null {
  if (!pending || Date.now() - pending.since >= KEEP_MS) return null;
  if (Date.now() - lastTry < RETRY_MS) return null;
  lastTry = Date.now();
  return pending.code;
}

/** The server's answer: done (confirmed, or no use trying), or try again. */
export function presenceAnswer(ok: boolean, reason?: string): "ok" | "retry" | "drop" {
  if (ok) {
    pending = null;
    save(null);
    return "ok";
  }
  if (reason === "far" || reason === "stale") return "retry";
  pending = null;
  save(null);
  return "drop";
}
