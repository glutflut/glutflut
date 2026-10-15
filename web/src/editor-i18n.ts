// Redaktion texts (editor-locales/, German the source), like i18n.ts for the
// app but in their own chunks: only the active language is loaded, and the
// game's bundle doesn't carry them. Language from lang.ts (?lang=, stored
// choice, browser).
import type { de } from "./editor-locales/de";
import { lang, type Lang } from "./lang";

export type EKey = keyof typeof de;
export type EDict = Record<EKey, string>;

const LOADERS: Record<Lang, () => Promise<EDict>> = {
  de: () => import("./editor-locales/de").then((m) => m.de),
  en: () => import("./editor-locales/en").then((m) => m.en),
  fr: () => import("./editor-locales/fr").then((m) => m.fr),
  es: () => import("./editor-locales/es").then((m) => m.es),
  pl: () => import("./editor-locales/pl").then((m) => m.pl),
  it: () => import("./editor-locales/it").then((m) => m.it),
  pt: () => import("./editor-locales/pt").then((m) => m.pt),
};

const dict: EDict = await LOADERS[lang]().catch(() => LOADERS.de());

/** Text for `key` with `{name}` placeholders filled in. */
export function t(key: EKey, vars: Record<string, string | number> = {}): string {
  return (dict[key] ?? key).replace(/\{(\w+)\}/g, (_, k) => String(vars[k] ?? ""));
}

/** Fill `data-i18n` (text), `data-i18n-html` (our own markup: <b>),
 * `data-i18n-title`, `data-i18n-aria` and `data-i18n-placeholder`. */
export function applyStatic(root: ParentNode = document) {
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n]")) el.textContent = t(el.dataset.i18n as EKey);
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-html]")) el.innerHTML = t(el.dataset.i18nHtml as EKey);
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle as EKey);
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-aria]")) el.setAttribute("aria-label", t(el.dataset.i18nAria as EKey));
  for (const el of root.querySelectorAll<HTMLElement>("[data-i18n-placeholder]")) el.setAttribute("placeholder", t(el.dataset.i18nPlaceholder as EKey));
  // "more"/"less" after the folded explanations (editor.css).
  document.documentElement.style.setProperty("--help-more", JSON.stringify(` ${t("ui.more")}`));
  document.documentElement.style.setProperty("--help-less", JSON.stringify(` ${t("ui.less")}`));
}
