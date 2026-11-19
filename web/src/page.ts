// Page-wide helpers: element lookup, the toast, the query string, short
// number formats.

export const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
export const params = new URLSearchParams(location.search);

// One toast at a time, each at least TOAST_MIN_MS; at most three wait (the
// oldest goes). While a big card is open (welcome, story, rating) they wait,
// and those older than TOAST_STALE_MS are dropped.
const TOAST_MIN_MS = 1500;
const TOAST_STALE_MS = 6000;
const TOAST_BLOCK = ["intro", "story", "sheet"];
const toasts: { text: string; ms: number; at: number }[] = [];
let toastTimer = 0;
let toastNext = 0;
let toastAt = 0;
export function toast(text: string, ms = 2200) {
  const el = $("toast");
  // The same again: just longer.
  if (!el.hidden && el.textContent === text) {
    clearTimeout(toastTimer);
    toastTimer = window.setTimeout(toastDone, ms);
    return;
  }
  toasts.push({ text, ms, at: Date.now() });
  if (toasts.length > 3) toasts.shift();
  toastPump();
}
function toastDone() {
  $("toast").hidden = true;
  toastPump();
}
function toastPump() {
  clearTimeout(toastNext);
  if (!toasts.length) return;
  const el = $("toast");
  const now = Date.now();
  if (!el.hidden && now - toastAt < TOAST_MIN_MS) return void (toastNext = window.setTimeout(toastPump, TOAST_MIN_MS - (now - toastAt)));
  if (TOAST_BLOCK.some((id) => document.getElementById(id)?.hidden === false)) {
    while (toasts.length && now - toasts[0].at > TOAST_STALE_MS) toasts.shift();
    if (toasts.length) toastNext = window.setTimeout(toastPump, 500);
    return;
  }
  const next = toasts.shift()!;
  el.textContent = next.text;
  el.hidden = false;
  toastAt = now;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(toastDone, next.ms);
}

// Moments other modules ask about ("a level card just came").
const marks = new Map<string, number>();
export const mark = (key: string) => void marks.set(key, Date.now());
export const markedWithin = (key: string, ms: number) => Date.now() - (marks.get(key) ?? 0) < ms;

export const plus = (n: number) => (n > 0 ? ` +${n}` : "");
export const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
