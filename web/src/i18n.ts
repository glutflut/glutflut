// Tiny i18n: one dictionary per language, German is the source. To add a
// language: copy locales/en.ts, translate, add it to LOADERS here and NAMES in
// lang.ts, and add a <article data-lang="xx"> block to info/datenschutz/impressum.html.
// Only the active language is loaded (own chunk); importers wait for it via
// top-level await, so nothing renders raw keys.
import type { de } from "./locales/de";
import { city, cityIn, fetchCity, lang, type Lang } from "./lang";

export { lang, langChosen, langSwitch, setLang, type Lang } from "./lang";

export type Key = keyof typeof de;
export type Dict = Record<Key, string>;

const LOADERS: Record<Lang, () => Promise<Dict>> = {
  de: () => import("./locales/de").then((m) => m.de),
  en: () => import("./locales/en").then((m) => m.en),
  fr: () => import("./locales/fr").then((m) => m.fr),
  es: () => import("./locales/es").then((m) => m.es),
  pl: () => import("./locales/pl").then((m) => m.pl),
  it: () => import("./locales/it").then((m) => m.it),
  pt: () => import("./locales/pt").then((m) => m.pt),
};

/** The dictionary for `l`; German if that chunk can't be loaded. */
async function loadDict(l: Lang): Promise<Dict> {
  try {
    return await LOADERS[l]();
  } catch (e) {
    if (l === "de") throw e;
    return LOADERS.de();
  }
}

// Every locale has all keys (`Dict`), so there is no runtime fallback.
let dict = await loadDict(lang);
let dictLang: Lang = lang;

/** "in Konstanz", or the region's own form ("im Furttal"), for `{in_city}`. */
export function inCity(name: string, forms?: Record<string, string> | null): string {
  return forms?.[lang] ?? dict["city.in"].replace("{city}", name);
}

/** `[one]… [few]… [other]…` in a text: the form for `count` by the
 *  language's plural rules, else `other`. */
function plural(text: string, count: number | undefined): string {
  if (!text.startsWith("[")) return text;
  const forms = Object.fromEntries([...text.matchAll(/\[(\w+)\]([^[]*)/g)].map((m) => [m[1], m[2]]));
  const cat = count === undefined ? "other" : new Intl.PluralRules(lang).select(count);
  return forms[cat] ?? forms.other ?? "";
}

/** Text for `key` with `{name}` placeholders filled in; `count` picks the
 *  plural form (when `n` is already formatted). */
export function t(key: Key, vars: Record<string, string | number> = {}): string {
  const all: Record<string, string | number> = { city, in_city: inCity(city, cityIn), ...vars };
  const count = typeof vars.count === "number" ? vars.count : typeof vars.n === "number" ? vars.n : undefined;
  return plural(dict[key] ?? key, count).replace(/\{(\w+)\}/g, (_, k) => String(all[k] ?? ""));
}

/** Ask the server for the city, then fill `.city` spans and call `done`. If
 *  the region's language replaces the default, its texts are loaded first. */
export function loadCity(done: () => void = () => {}) {
  fetchCity()
    .then(async () => {
      if (lang !== dictLang) {
        const l = lang;
        dict = await loadDict(l);
        dictLang = l;
      }
      done();
    })
    .catch(() => {});
}

/** Text for a key built at runtime (e.g. from server data), else `fallback`. */
export function tOr(key: string, fallback: string): string {
  return key in dict ? t(key as Key) : fallback;
}

/** Fill `data-i18n` (text), `data-i18n-title`, `data-i18n-aria` (aria-label) and `data-i18n-placeholder`. */
export function applyStatic(root: ParentNode = document) {
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n]")) el.textContent = t(el.dataset.i18n as Key);
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle as Key);
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-aria]")) el.setAttribute("aria-label", t(el.dataset.i18nAria as Key));
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-placeholder]")) el.setAttribute("placeholder", t(el.dataset.i18nPlaceholder as Key));
}
