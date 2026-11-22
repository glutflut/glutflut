// Planned maintenance (server: maintenance.rs, admin page): from 48 h before a
// bar above the buttons (dismissable once per window), a toast 10 minutes
// before, a line under "what's new", and while it runs an overlay instead of
// "Verbinde …" once the connection is gone. During maintenance the proxy
// answers `/api/maintenance` itself with 503 `{"maintenance": true, "until"}`.
import { lang, t } from "./i18n";
import { $ } from "./page";
import { KEYS, load, save } from "./store";

type Window = { id: number; starts: number; ends: number; note: string | null };
type Reply = { maintenance: boolean; window?: Window | null; tz?: string | null; until?: string | null };

const DISMISSED = KEYS.maintDismissed;
const WARNED = KEYS.maintWarned;
const POLL_MS = 5 * 60_000;
const DOWN_POLL_MS = 30_000;
/** Disconnected this long before asking whether it is maintenance. */
const LOST_MS = 4_000;

let win: Window | null = null;
let tz: string | undefined;

const fmt = (t: number, o: Intl.DateTimeFormatOptions) => {
  try {
    return new Intl.DateTimeFormat(lang, { ...o, timeZone: tz }).format(t * 1000);
  } catch {
    return new Intl.DateTimeFormat(lang, o).format(t * 1000);
  }
};
const hm = (t: number) => fmt(t, { hour: "2-digit", minute: "2-digit" });
/** "Sa. 22.11., 03:00–03:30" in the region's time. */
const span = (w: Window) => `${fmt(w.starts, { weekday: "short", day: "numeric", month: "numeric" })}, ${hm(w.starts)}–${hm(w.ends)}`;
const noteText = (w: Window) => (w.note === "move" ? t("maint.note.move") : w.note === "update" ? t("maint.note.update") : w.note === "database" ? t("maint.note.database") : "");

/** The announced window as one sentence (news), or null. */
export function maintenanceLine(): string | null {
  if (!win || win.ends * 1000 < Date.now()) return null;
  return [t("maint.banner", { when: span(win) }), noteText(win)].filter(Boolean).join(" ");
}

export function setupMaintenance(region: () => string, toast: (text: string, ms?: number) => void) {
  const bar = $("maint-bar");
  const overlay = $("maint-overlay");
  let connected = false;
  let lostTimer = 0;
  let warnTimer = 0;
  let downTimer = 0;

  /** The bar; in the last 10 minutes again even if closed (a toast may
   *  wait behind an open card). */
  function showBar() {
    const now = Date.now() / 1000;
    const soon = !!win && win.starts - 600 <= now;
    const on = !!win && win.ends > now && (soon || load(DISMISSED) !== String(win.id));
    bar.hidden = !on;
    if (on && win) $("maint-text").textContent = soon ? t("maint.soon") : t("maint.banner", { when: span(win) });
  }

  function scheduleWarn() {
    clearTimeout(warnTimer);
    if (!win || load(WARNED) === String(win.id)) return;
    const w = win;
    const at = (w.starts - 600) * 1000 - Date.now();
    if (w.starts * 1000 < Date.now()) return;
    warnTimer = window.setTimeout(() => {
      save(WARNED, String(w.id));
      showBar();
      toast(t("maint.soon"), 8000);
    }, Math.max(0, at));
  }

  function showOverlay(until: number | null) {
    $("maint-until").textContent = until ? t("maint.until", { time: hm(until) }) : t("maint.back");
    overlay.hidden = false;
    clearTimeout(downTimer);
    downTimer = window.setTimeout(() => void ask(), DOWN_POLL_MS);
  }

  function hideOverlay() {
    overlay.hidden = true;
    clearTimeout(downTimer);
  }

  async function ask() {
    const res = await fetch(`/api/maintenance?region=${encodeURIComponent(region())}`, { cache: "no-store" }).catch(() => null);
    const j = res ? ((await res.json().catch(() => null)) as Reply | null) : null;
    if (j?.maintenance) {
      // The proxy's page: the server is switched off for maintenance.
      const until = j.until ? Date.parse(j.until) / 1000 : (win?.ends ?? null);
      if (!connected) showOverlay(Number.isFinite(until) ? until : null);
      return;
    }
    if (j && res?.ok) {
      win = j.window ?? null;
      tz = j.tz ?? undefined;
      showBar();
      scheduleWarn();
    }
    const now = Date.now() / 1000;
    // No answer at all during an announced window: maintenance as well.
    if (!connected && !j && win && win.starts - 120 <= now && now < win.ends + 3600) return showOverlay(win.ends);
    if (connected || j) hideOverlay();
    else if (!overlay.hidden) downTimer = window.setTimeout(() => void ask(), DOWN_POLL_MS);
  }

  $("maint-close").onclick = () => {
    if (win) save(DISMISSED, String(win.id));
    bar.hidden = true;
  };

  void ask();
  setInterval(() => !document.hidden && void ask(), POLL_MS);

  return {
    /** From the connection (net.ts status). */
    onStatus(on: boolean) {
      connected = on;
      clearTimeout(lostTimer);
      if (on) hideOverlay();
      else lostTimer = window.setTimeout(() => !connected && void ask(), LOST_MS);
    },
  };
}
