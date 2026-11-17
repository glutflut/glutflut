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

/** Cards waiting in `whenFree`: an earlier one in CARDS goes first. */
const waiting = new Map<Card, number>();
const ahead = (self: Card) => [...waiting.keys()].some((c) => CARDS.indexOf(c) < CARDS.indexOf(self));

/** Run `fn` once no other card than `self` is open and none before it in
 *  CARDS waits (checked twice a second); gives up after `tries` checks. */
export function whenFree(self: Card, fn: () => void, tries = Infinity) {
  waiting.set(self, (waiting.get(self) ?? 0) + 1);
  const leave = () => {
    const n = (waiting.get(self) ?? 1) - 1;
    if (n) waiting.set(self, n);
    else waiting.delete(self);
  };
  const go = () => {
    if (cardOpen(self) || ahead(self)) {
      if (--tries > 0) window.setTimeout(go, 500);
      else leave();
      return;
    }
    leave();
    fn();
  };
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
