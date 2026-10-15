// Static pages: show the <article data-lang> for the current language
// (German if missing) and its title, plus the language buttons. Shown again
// if the region's language replaces the default (see i18n `loadCity`).
import { lang, langChosen, langSwitch, loadCity } from "./lang";

function show() {
  const blocks = [...document.querySelectorAll<HTMLElement>("article[data-lang]")];
  const pick = blocks.find((b) => b.dataset.lang === lang) ?? blocks.find((b) => b.dataset.lang === "de");
  for (const b of blocks) b.hidden = b !== pick;
  if (pick) {
    document.documentElement.lang = pick.dataset.lang!;
    if (pick.dataset.title) document.title = pick.dataset.title;
  }
  langSwitch();
}

// Links to the game keep an explicitly chosen language.
if (langChosen) {
  for (const a of document.querySelectorAll<HTMLAnchorElement>('a[href^="/"]')) {
    const u = new URL(a.href);
    if (!u.pathname.startsWith("/api")) {
      u.searchParams.set("lang", lang);
      a.href = u.pathname + u.search + u.hash;
    }
  }
}
// Inside the game's overlay (pages.ts): "back to the game" closes it, other
// pages open in place (no history entries, so back closes the overlay).
if (window.parent !== window) {
  document.addEventListener("click", (e) => {
    const a = (e.target as Element).closest?.("a");
    if (!a || a.target === "_blank") return;
    const u = new URL(a.href, location.href);
    if (u.origin !== location.origin || u.pathname.startsWith("/api")) return;
    e.preventDefault();
    if (u.pathname === "/" || u.pathname === "/index.html") window.parent.postMessage({ t: "elements:close" }, location.origin);
    else location.replace(u.href);
  });
}
show();
loadCity(show);
