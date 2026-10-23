// The story card: a longer text with a title, a kicker and maybe an image
// (chapter intros and endings, letters found, news flashes). One at a time;
// more wait their turn. What was shown is remembered per device (seen).
import { $ } from "./page";
import { t } from "./i18n";
import { figure, mediaFor, type Media } from "./media";
import { KEYS, loadJson, saveJson } from "./store";

export interface Story {
  /** `fire`, `water`, `badge` (gold) or `notice`: the card's colour. */
  cls: string;
  kicker: string;
  title: string;
  /** A small line under the title (a collection's progress, a date). */
  line?: string;
  /** Paragraphs separated by blank lines. */
  text: string;
  media?: [Media["kind"], string];
  /** A link under the text, with its label. */
  url?: string;
  urlLabel?: string;
}

const queue: (Story & { done?: () => void })[] = [];
let open = false;
let shown = 0;

/** Paragraphs as <p>, nothing else (texts come from the city, not markup). */
export function paragraphs(text: string): HTMLElement[] {
  return text
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const el = document.createElement("p");
      const lines = p.split("\n");
      lines.forEach((l, i) => {
        if (i) el.append(document.createElement("br"));
        el.append(l);
      });
      return el;
    });
}

function render(s: Story & { done?: () => void }) {
  const box = $("story");
  open = true;
  box.className = `card story ${s.cls}`;
  $("story-kicker").textContent = s.kicker;
  $("story-title").textContent = s.title;
  $("story-line").textContent = s.line ?? "";
  $("story-line").hidden = !s.line;
  $("story-text").replaceChildren(...paragraphs(s.text));
  const link = $("story-link");
  const a = link.querySelector("a")!;
  link.hidden = !s.url;
  if (s.url) {
    a.href = s.url;
    a.textContent = s.urlLabel ?? s.url;
  }
  const slot = $("story-media");
  slot.replaceChildren();
  const n = ++shown;
  if (s.media)
    void mediaFor(...s.media).then((m) => {
      if (m && n === shown) slot.append(figure(m));
    });
  $("story-ok").textContent = t("story.ok");
  box.hidden = false;
  box.scrollTop = 0;
  const close = () => {
    if (n !== shown) return;
    box.hidden = true;
    open = false;
    s.done?.();
    const next = queue.shift();
    if (next) setTimeout(() => render(next), 300);
  };
  $("story-close").onclick = close;
  $("story-ok").onclick = close;
}

/** Show a story card (after the ones before it, and after the welcome
 * card of a first visit); `done` runs when it closes. */
export function showStory(s: Story, done?: () => void) {
  const item = { ...s, done };
  if (open) return queue.push(item);
  open = true;
  const start = () => {
    if ($("intro").hidden) return render(item);
    setTimeout(start, 500);
  };
  start();
}

/** Marks of what this device has seen (chapter intros, endings), newest last. */
function seenList(): string[] {
  return loadJson<string[]>(KEYS.storySeen) ?? [];
}

export function storySeen(mark: string): boolean {
  return seenList().includes(mark);
}

export function markStorySeen(mark: string) {
  const list = seenList().filter((m) => m !== mark);
  list.push(mark);
  saveJson(KEYS.storySeen, list.slice(-60));
}

/** Text in the app's language, else the city's, English, any. */
export function pickLang<T>(names: Record<string, T> | null | undefined, lang: string): T | undefined {
  if (!names) return undefined;
  return names[lang] ?? names.de ?? names.en ?? Object.values(names)[0];
}
