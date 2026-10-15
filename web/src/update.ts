// New version after a deploy. An app on the home screen is hardly ever
// reloaded (no browser bar, suspended for days), but every deploy restarts the
// server, so every app reconnects and gets `welcome` with the server's
// version. Different from ours: reload when the app comes back to the
// foreground (or at once if it is in the background), meanwhile a bar to tap.
// Settings have "App neu laden" for testing and support.
import { VERSION } from "./version";
import { $ } from "./page";

const KEY = "elements.reloaded-for";
/** Image builds only: locally the server says "dev" and the app "(lokal)". */
const real = (v: string | undefined): v is string => !!v && v !== "dev" && !v.includes("(lokal)");

/** `busy()`: a dialog or card is open, don't pull the page away now. */
export function setupUpdate(busy: () => boolean) {
  const bar = $("update") as HTMLButtonElement;
  let pending: string | null = null;

  function reload() {
    try {
      // Once per version: a page that comes back stale (cache) doesn't loop.
      if (pending) sessionStorage.setItem(KEY, pending);
    } catch {
      // storage blocked: reload anyway
    }
    location.reload();
  }

  bar.onclick = reload;
  ($("reload") as HTMLButtonElement).onclick = () => location.reload();
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && pending && !busy()) reload();
  });

  return {
    /** True: this app is outdated (reloading, or the bar to tap). */
    onServerVersion(v: string | undefined): boolean {
      if (!real(v) || !real(VERSION) || v === VERSION) return false;
      bar.hidden = false;
      let tried = false;
      try {
        tried = sessionStorage.getItem(KEY) === v;
      } catch {
        tried = true;
      }
      if (tried) return true; // reloaded once already: the bar only
      pending = v;
      if (document.hidden && !busy()) reload();
      return true;
    },
  };
}
