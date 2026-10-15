// Tag line texts: built-in phrases from the locale files, the city's
// campaign phrases (`c-<n>`, editor) from the welcome message.
import { lang, t, type Key } from "./i18n";
import type { MottoItem } from "./protocol";

let city = new Map<string, Record<string, string>>();

export function setMottos(list: MottoItem[]) {
  city = new Map(list.filter((m) => m.names).map((m) => [m.key, m.names!]));
}

/** The phrase in the app's language (city phrases: else German, English, any; unknown: ""). */
export function mottoText(key: string): string {
  if (!key.startsWith("c-")) return t(`motto.${key}` as Key);
  const n = city.get(key);
  return n ? (n[lang] ?? n.de ?? n.en ?? Object.values(n)[0] ?? "") : "";
}
