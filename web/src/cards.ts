// One card at a time: the welcome, story cards (letters, chapters), find
// cards and the rating sheet wait for each other. Order when several are
// due: letter > find > rating > badge (badges are mostly toasts now).
import { $ } from "./page";

const CARDS = ["intro", "story", "find-card", "sheet", "profile-ask"] as const;
type Card = (typeof CARDS)[number];

/** Another card than `self` is open. */
export function cardOpen(self?: Card): boolean {
  return CARDS.some((id) => id !== self && !$(id).hidden);
}

/** Run `fn` once no other card than `self` is open (checked twice a second). */
export function whenFree(self: Card, fn: () => void, wait = 500) {
  const go = () => (cardOpen(self) ? window.setTimeout(go, wait) : fn());
  go();
}

let yieldSheet: (() => void) | null = null;
/** The rating sheet, when it asked by itself, steps back for a letter. */
export function onYield(fn: () => void) {
  yieldSheet = fn;
}
export function preempt() {
  yieldSheet?.();
}
