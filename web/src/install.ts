// Install hint (PWA). Browsers offer installing only by their own rules (and
// not again for a while after an uninstall), so from the second day of play a
// small card does: Chrome/Edge get the browser's dialog (beforeinstallprompt),
// iPhone/iPad the two steps via Share, other browsers the menu entry. "Not
// now" hides it for 14 days; an installed app never sees it. Installed apps
// also keep their storage (token, tickets) more reliably.
import { t } from "./i18n";
import { $ } from "./page";
import { KEYS, load, save } from "./store";

type PromptEvent = Event & { prompt(): Promise<void>; userChoice: Promise<{ outcome: string }> };

const LATER_MS = 14 * 86_400_000;
const today = () => Math.floor(Date.now() / 86_400_000);

let deferred: PromptEvent | null = null;
addEventListener("beforeinstallprompt", (e) => {
  e.preventDefault();
  deferred = e as PromptEvent;
});
addEventListener("appinstalled", () => {
  deferred = null;
  $("install").hidden = true;
  save(KEYS.install, JSON.stringify({ since: 0, until: Number.MAX_SAFE_INTEGER }));
});

const installed = () =>
  matchMedia("(display-mode: standalone)").matches || (navigator as Navigator & { standalone?: boolean }).standalone === true;
const ios = () => /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);

function state(): { since: number; until: number } {
  try {
    const s = JSON.parse(load(KEYS.install) ?? "");
    if (typeof s.since === "number" && typeof s.until === "number") return s;
  } catch {
    // first visit or broken value
  }
  const s = { since: today(), until: 0 };
  save(KEYS.install, JSON.stringify(s));
  return s;
}

/** After a welcome: maybe show the card (once per visit, a little later). */
let checked = false;
export function checkInstall() {
  if (checked) return;
  checked = true;
  if (installed() || !matchMedia("(pointer: coarse)").matches) return;
  const s = state();
  if (today() <= s.since || Date.now() < s.until) return;
  setTimeout(() => {
    if (!$("news").hidden || installed()) return;
    const box = $("install");
    const go = $<HTMLButtonElement>("install-go");
    $("install-text").textContent = deferred ? t("install.text") : ios() ? t("install.ios") : t("install.menu");
    go.hidden = !deferred;
    go.onclick = async () => {
      const e = deferred;
      if (!e) return;
      deferred = null;
      box.hidden = true;
      await e.prompt().catch(() => {});
      // Declined in the browser's dialog: ask again in 14 days.
      const r = await e.userChoice.catch(() => ({ outcome: "dismissed" }));
      if (r.outcome !== "accepted") save(KEYS.install, JSON.stringify({ ...s, until: Date.now() + LATER_MS }));
    };
    $("install-later").onclick = () => {
      box.hidden = true;
      save(KEYS.install, JSON.stringify({ ...s, until: Date.now() + LATER_MS }));
    };
    box.hidden = false;
  }, 20_000);
}
