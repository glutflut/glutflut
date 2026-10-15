// Top of the screen: the player bar (#hud) folds to a small pill, the chapter
// bar below it to one line. Kept per device; a chapter stays folded until the
// next one. The map buttons follow whatever is left.
import { $ } from "./page";
import { KEYS, load, save } from "./store";

/** Chapter bar below the header, map buttons below both (via `--card-top`,
 *  which counts from the safe-area inset). */
export function layoutTop() {
  const hud = $("hud");
  const r = hud.getBoundingClientRect();
  // The header sits at the inset plus its margin.
  const inset = r.top - parseFloat(getComputedStyle(hud).marginTop);
  let bottom = r.bottom;
  const bar = $("chapter");
  if (!bar.hidden) {
    bar.style.setProperty("--card-top", `${bottom + 6 - inset}px`);
    bottom = bar.getBoundingClientRect().bottom;
  }
  $("nav").style.setProperty("--card-top", `${bottom + 10 - inset}px`);
}

export function setupTop(label: (key: "top.fold" | "top.unfold") => string) {
  const hud = $("hud");
  const btn = $("fold");
  const set = (on: boolean) => {
    hud.classList.toggle("folded", on);
    btn.setAttribute("aria-expanded", `${!on}`);
    btn.title = label(on ? "top.unfold" : "top.fold");
    btn.setAttribute("aria-label", btn.title);
    layoutTop();
  };
  set(load(KEYS.topFolded) === "1");
  btn.onclick = () => {
    const on = !hud.classList.contains("folded");
    save(KEYS.topFolded, on ? "1" : "0");
    set(on);
  };
  const ro = new ResizeObserver(() => layoutTop());
  ro.observe(hud);
  ro.observe($("chapter"));
  addEventListener("resize", layoutTop);
}
