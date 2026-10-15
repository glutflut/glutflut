// Redaktion: tabs, short notices (toasts, with undo), a confirm dialog,
// server errors in the page's language, and a guard for unsaved forms.

import { NAMES, type Lang } from "./lang";
import { t, type EKey } from "./editor-i18n";

const $ = (id: string) => document.getElementById(id) as HTMLElement;

// --- Languages and dates ----------------------------------------------------

/** The app's languages (lang.ts), with their own names. */
export const LANGS = Object.keys(NAMES) as Lang[];
export const langName = (l: string) => NAMES[l as Lang] ?? l;
/** The page's language, for dates. */
const loc = () => document.documentElement.lang || "de";
/** One date format everywhere: "14. Okt. 2026, 09:00" (`time` false: without time). */
export const fmtDate = (s: number, time = true) =>
  new Date(s * 1000).toLocaleString(loc(), time ? { dateStyle: "medium", timeStyle: "short" } : { dateStyle: "medium" });
export const fmtTime = (s: number) => new Date(s * 1000).toLocaleTimeString(loc(), { timeStyle: "short" });
/** Unix seconds <-> the value of a datetime-local input (browser's time zone). */
export function toLocal(s: number): string {
  const d = new Date(s * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}
export const fromLocal = (v: string) => Math.round(new Date(v).getTime() / 1000);
/** Inside the page root: its colour variables apply. */
const root = () => document.querySelector<HTMLElement>(".viz-root") ?? document.body;

/** Collections as the app names them (its locales `finds.c.*`). */
export const COLL_NAME: Record<string, string> = Object.fromEntries(
  (["fountains", "trees", "gates", "views", "art", "stones"] as const).map((c) => [c, t(`coll.${c}`)]),
);

// --- Server errors ----------------------------------------------------------

/** The server's messages (editor*.rs, media.rs) and their texts. */
const SERVER: Record<string, EKey> = {
  "already decided": "err.decided",
  "another site is on that junction": "err.site.taken",
  "approving needs an admin token": "err.admin",
  "a second person approves (not the uploader)": "err.second",
  "at most 31 days": "err.days31",
  "at most 40 characters": "err.chars40",
  "at most 8 phrases per campaign and side": "err.phrases8",
  "author: 1-120 characters": "err.author",
  "alt text: at most 200 characters": "err.alt",
  "campaign: 3-40 of a-z, 0-9, -": "err.campaign",
  "collection unknown": "err.collection",
  "control characters": "err.control",
  "database unavailable": "err.db",
  "ends before it starts": "err.order",
  "file unreadable": "err.file",
  "image larger than 8 MB": "err.big",
  "image too small (at least 200 x 150 px)": "err.small",
  "JPEG, PNG or WebP only": "err.format",
  "licence: own, cc-by-4.0, cc-by-sa-4.0 or cc0-1.0": "err.licence",
  "media: commons fetch": "err.commons",
  "media: upload failed": "err.upload",
  "media: delete failed": "err.mdelete",
  "name: 1-60 characters": "err.name",
  "no chapter of this campaign yet": "err.nochapter",
  "no junction within 150 m": "err.nojunction",
  "no name on this token": "err.noname",
  "no street within 40 m": "err.nostreet",
  "no such draft or future chapter": "err.nochap",
  "no such image": "err.noimage",
  "no such phrase": "err.nophrase",
  "no such place": "err.noplace",
  "no such plan, or it is over": "err.noplan",
  "not a Commons file": "err.notcommons",
  "not a suggestion for this find": "err.notsugg",
  "outside the region": "err.outside",
  "overlaps another chapter": "err.overlap",
  "plan: 10 minutes to 12 hours": "err.plan.len",
  "plan: hotspot site missing": "err.plan.site",
  "plan: start and end missing": "err.plan.times",
  "plan: the start is in the past": "err.past",
  "plans: delete and plan anew": "err.plan.change",
  "position invalid": "err.pos",
  "radius: 30-1000 m": "err.radius",
  "source: an https:// link": "err.source",
  "started: only titles and texts can change": "err.started",
  "starts in the past": "err.past",
  "target: 1 to 1,000,000": "err.target",
  "the text in the city's language is required": "err.primary.text",
  "title and text, both or neither": "err.both",
  "title and text in the city's language are required": "err.primary",
  "too long": "err.long",
  "too many finds (400)": "err.max.finds",
  "too many hotspot sites (30)": "err.max.sites",
  "too many images waiting for approval": "err.max.pending",
  "too many planned hotspots (50)": "err.max.plans",
  "too many zones (100)": "err.max.zones",
  "unknown goal": "err.goal",
  "unknown target": "err.target.unknown",
};

/** The error of a failed response, translated (`fallback` without one). */
export async function errorOf(res: Response | null | undefined, fallback: string): Promise<string> {
  if (!res) return t("err.nonet");
  if (res.status === 401 || res.status === 403) return t("err.denied");
  const msg: string | undefined = (await res.json().catch(() => null))?.error;
  if (!msg) return fallback;
  const key = SERVER[msg];
  return key ? t(key) : /^no such /.test(msg) ? t("err.gone") : `${fallback} (${msg})`;
}

// --- Toasts -----------------------------------------------------------------

let box: HTMLElement | null = null;

/** A short notice at the bottom; errors stay longer. `undo`: a button that runs it. */
export function toast(text: string, kind: "ok" | "error" = "ok", undo?: () => void) {
  box ??= root().appendChild(Object.assign(document.createElement("div"), { className: "ed-toasts" }));
  box.setAttribute("role", "status");
  box.setAttribute("aria-live", "polite");
  const n = document.createElement("div");
  n.className = `ed-toast ${kind}`;
  n.append(text);
  const gone = () => n.remove();
  if (undo) {
    const b = Object.assign(document.createElement("button"), { type: "button", textContent: t("ui.undo") });
    b.onclick = () => {
      gone();
      undo();
    };
    n.append(b);
  }
  box.append(n);
  setTimeout(gone, kind === "error" ? 9000 : undo ? 8000 : 3500);
}

// --- Confirm ----------------------------------------------------------------

/** A confirm dialog; resolves true on `yes`. */
export function ask(text: string, yes = t("ui.ok"), danger = false): Promise<boolean> {
  const d = document.createElement("dialog");
  d.className = "ed-dialog";
  const p = Object.assign(document.createElement("p"), { textContent: text });
  const row = Object.assign(document.createElement("div"), { className: "ed-row" });
  const ok = Object.assign(document.createElement("button"), { type: "button", textContent: yes, className: danger ? "danger" : "primary" });
  const no = Object.assign(document.createElement("button"), { type: "button", textContent: t("ui.cancel") });
  row.append(ok, no);
  d.append(p, row);
  root().append(d);
  d.showModal();
  no.focus();
  return new Promise((done) => {
    const end = (v: boolean) => {
      d.close();
      d.remove();
      done(v);
    };
    ok.onclick = () => end(true);
    no.onclick = () => end(false);
    d.oncancel = (e) => {
      e.preventDefault();
      end(false);
    };
  });
}

// --- Unsaved forms ----------------------------------------------------------

const dirty = new Set<string>();

/** Mark the form `id` changed on any input; `clean(id)` after saving or closing. */
export function watchForm(id: string) {
  const f = $(id);
  const on = () => {
    if (!f.hidden && !f.closest("[hidden]")) dirty.add(id);
  };
  f.addEventListener("input", on);
  f.addEventListener("change", on);
}
export const clean = (id: string) => dirty.delete(id);
/** True when nothing is unsaved, or the user agrees to drop it. */
export async function mayLeave(id?: string): Promise<boolean> {
  const open = id ? dirty.has(id) : dirty.size > 0;
  if (!open) return true;
  if (!(await ask(t("ui.discard.ask"), t("ui.discard"), true))) return false;
  if (id) dirty.delete(id);
  else dirty.clear();
  return true;
}
addEventListener("beforeunload", (e) => {
  if (dirty.size) e.preventDefault();
});

// --- Tabs -------------------------------------------------------------------

type Tab = { id: string; button: HTMLButtonElement; panel: HTMLElement };
let tabs: Tab[] = [];
let onShow: Record<string, () => void> = {};

/** `nav#tabs` buttons with `data-tab` show the `[data-panel]` of that name; the
 * address keeps the tab (`#bilder`), so reload and back work. */
export function setupTabs(shown: Record<string, () => void> = {}) {
  onShow = shown;
  tabs = [...document.querySelectorAll<HTMLButtonElement>("#tabs button[data-tab]")].map((button) => ({
    id: button.dataset.tab!,
    button,
    panel: document.querySelector<HTMLElement>(`[data-panel="${button.dataset.tab}"]`)!,
  }));
  for (const t of tabs) t.button.onclick = () => (location.hash = t.id);
  addEventListener("hashchange", () => show(location.hash.slice(1)));
  show(location.hash.slice(1));
}

function show(id: string) {
  const want = tabs.find((t) => t.id === id && !t.button.hidden) ?? tabs.find((t) => !t.button.hidden) ?? tabs[0];
  if (!want) return;
  for (const t of tabs) {
    const on = t === want;
    t.panel.hidden = !on;
    t.button.classList.toggle("on", on);
    t.button.setAttribute("aria-selected", String(on));
  }
  onShow[want.id]?.();
}

/** A number on a tab, e.g. images waiting for approval ("" for none). */
export function setBadge(tab: string, text: string) {
  const b = document.querySelector(`#tabs button[data-tab="${tab}"] .badge`);
  if (b) b.textContent = text;
}

/** A tab only when one of its cards is there (cards hide when their data can't load). */
export function syncTabs() {
  for (const t of tabs) {
    t.button.hidden = ![...t.panel.querySelectorAll<HTMLElement>("section.card")].some((s) => !s.hidden || s.dataset.always !== undefined);
  }
  const cur = tabs.find((t) => t.button.classList.contains("on"));
  if (!cur || cur.button.hidden) show(location.hash.slice(1));
}
