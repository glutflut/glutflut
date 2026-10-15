// In-app question instead of the browser's confirm() (blocking, outside the
// app's look): a card with the text, "Abbrechen" and "Ja".
import { $ } from "./page";

let answer: ((yes: boolean) => void) | null = null;

function close(yes: boolean) {
  const a = answer;
  answer = null;
  $("ask").hidden = true;
  a?.(yes);
}

/** Show `text`; true on "Ja". A new question answers an open one with no. */
export function ask(text: string): Promise<boolean> {
  if (answer) close(false);
  $("ask-text").textContent = text;
  $("ask").hidden = false;
  $("ask-yes").onclick = () => close(true);
  $("ask-no").onclick = () => close(false);
  return new Promise((r) => (answer = r));
}
