// Help and legal pages inside the game: an overlay with the page in a frame,
// so going back closes it instead of reloading the game (position, map and
// connection stay). The pages' own "back to the game" link (pagelang.ts
// sends it here) and the phone's back button close it.
import { isLang, lang, setLang } from "./lang";
import { withRegion } from "./region";
import { $ } from "./page";

const PAGES = /^\/(info|impressum|datenschutz)\.html$/;

export function setupPages() {
  const box = $("page");
  const frame = $("page-frame") as HTMLIFrameElement;
  // Language picked in the overlay; the game reloads in it on close.
  let newLang: string | null = null;

  // Replace, never add: frame navigations would otherwise land in the
  // history, and back would step through them instead of closing.
  function load(url: string) {
    const w = frame.contentWindow;
    if (w) w.location.replace(url);
    else frame.src = url;
  }
  function hide() {
    if (box.hidden) return;
    box.hidden = true;
    load("about:blank");
    if (newLang && newLang !== lang) setLang(newLang);
  }
  // One history entry while open: back (button, gesture) closes.
  function close() {
    if (history.state?.page) history.back();
    else hide();
  }

  document.addEventListener(
    "click",
    (e) => {
      const a = (e.target as Element).closest?.("a");
      if (!a || e.ctrlKey || e.metaKey || e.shiftKey || a.target === "_blank") return;
      const u = new URL(a.href, location.href);
      if (u.origin !== location.origin || !PAGES.test(u.pathname)) return;
      e.preventDefault();
      load(withRegion(u.pathname + u.search) + u.hash);
      if (box.hidden) history.pushState({ ...history.state, page: true }, "");
      box.hidden = false;
    },
    true,
  );
  addEventListener("popstate", hide);
  addEventListener("message", (e) => {
    if (e.origin !== location.origin) return;
    if (e.data?.t === "elements:close") close();
    else if (e.data?.t === "elements:lang" && isLang(e.data.lang)) newLang = e.data.lang;
  });
}
