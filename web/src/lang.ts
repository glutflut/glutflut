// The UI language and the instance's city, without any dictionary: the static
// pages (pagelang.ts) need only this. i18n.ts loads the texts on top.
import { withRegion } from "./region";
import { KEYS, load, save } from "./store";

/** Supported languages with their own names, for the dropdown. Keep in sync
 *  with `lang.name` in locales/ and the loaders in i18n.ts. */
export const NAMES = { de: "Deutsch", en: "English", fr: "Français", es: "Español", pl: "Polski", it: "Italiano", pt: "Português" };
export type Lang = keyof typeof NAMES;
export const isLang = (l: unknown): l is Lang => typeof l === "string" && l in NAMES;

const KEY = KEYS.lang;

/** ?lang= beats the stored choice beats the browser's languages; if none of
 *  them is supported, the region's language (from `/api/version`, see
 *  `fetchCity`), German until that answer arrives. */
function detect(): { lang: Lang; chosen: boolean; fallback: boolean } {
  const stored = load(KEY);
  const url = new URLSearchParams(location.search).get("lang");
  const chosen = isLang(url) ? url : isLang(stored) ? stored : null;
  if (chosen) return { lang: chosen, chosen: true, fallback: false };
  const browser = navigator.languages.map((l) => l.slice(0, 2).toLowerCase()).find(isLang);
  return browser ? { lang: browser, chosen: false, fallback: false } : { lang: "de", chosen: false, fallback: true };
}

const detected = detect();
export let lang: Lang = detected.lang;
/** The language was picked explicitly (?lang= or the language buttons): links
 *  pass it on. Otherwise every page detects it the same way. */
export const langChosen = detected.chosen;
document.documentElement.lang = lang;

/** City of this instance (`/api/version`), for `{city}` in texts. */
export let city = "Konstanz";
/** The region's own "in …" per language ("im Furttal"), if it sets one. */
export let cityIn: Record<string, string> | null = null;

/** Ask the server for the city (and, with no usable browser language, take
 *  the region's language), then fill `.city` spans. Rejects on failure. */
export async function fetchCity(): Promise<void> {
  const v = await (await fetch(withRegion("/api/version"))).json();
  if (typeof v.city === "string" && v.city) city = v.city;
  if (v.city_in && typeof v.city_in === "object") cityIn = v.city_in;
  if (detected.fallback && isLang(v.language) && v.language !== lang) {
    lang = v.language;
    document.documentElement.lang = lang;
  }
  for (const el of document.querySelectorAll<HTMLElement>(".city")) el.textContent = city;
}

/** `fetchCity`, then `done`; errors are ignored. */
export function loadCity(done: () => void = () => {}) {
  fetchCity().then(done, () => {});
}

export function setLang(l: string) {
  save(KEY, l);
  // In the game's overlay (pages.ts): the game switches too, on close.
  if (window.parent !== window) window.parent.postMessage({ t: "elements:lang", lang: l }, location.origin);
  const u = new URL(location.href);
  u.searchParams.set("lang", l);
  location.replace(u);
}

/** Language dropdown into every `.lang-switch` (a list grows with more languages). */
export function langSwitch(root: ParentNode = document) {
  for (const host of root.querySelectorAll<HTMLElement>(".lang-switch")) {
    const sel = document.createElement("select");
    sel.setAttribute("aria-label", host.getAttribute("aria-label") ?? "Sprache / Language");
    sel.append(
      ...Object.entries(NAMES)
        .sort((a, b) => a[1].localeCompare(b[1]))
        .map(([code, name]) => Object.assign(new Option(name, code), { lang: code })),
    );
    sel.value = lang;
    sel.onchange = () => sel.value !== lang && setLang(sel.value);
    host.replaceChildren(sel);
  }
}
