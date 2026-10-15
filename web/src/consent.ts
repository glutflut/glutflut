// Consent and objection, kept in this browser only. "16 or older" unlocks the
// opt-ins (finds, optional questions; Art. 8 GDPR); "count for the city" can be
// switched off (objection, Art. 21 GDPR) without leaving the game.
import { t } from "./i18n";
import { KEYS, load, save } from "./store";
import { ask } from "./ask";

const ADULT_KEY = KEYS.adult;
const STATS_KEY = KEYS.stats;

const adultListeners: ((on: boolean) => void)[] = [];

export const isAdult = () => load(ADULT_KEY) === "1";

export function setAdult(on: boolean) {
  save(ADULT_KEY, on ? "1" : null);
  for (const f of adultListeners) f(on);
}

/** Called when "16 or older" changes. */
export const onAdult = (f: (on: boolean) => void) => adultListeners.push(f);

/** True if already confirmed, or the player confirms now. */
export async function confirmAdult(): Promise<boolean> {
  if (isAdult()) return true;
  if (!(await ask(t("age.confirm")))) return false;
  setAdult(true);
  return true;
}

/** Counted in the analytics for the city (default). */
export const isCounted = () => load(STATS_KEY) !== "0";
export const setCounted = (on: boolean) => save(STATS_KEY, on ? null : "0");
