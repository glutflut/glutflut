// Page-wide helpers: element lookup, the toast, the query string, short
// number formats.

export const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
export const params = new URLSearchParams(location.search);

let toastTimer = 0;
export function toast(text: string, ms = 2200) {
  const el = $("toast");
  el.textContent = text;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (el.hidden = true), ms);
}

export const plus = (n: number) => (n > 0 ? ` +${n}` : "");
export const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
