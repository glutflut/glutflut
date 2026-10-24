// Redaktion (server: editor.rs): a city's chapters, written by a token or a
// login (opsauth.ts) with the role "editor". Page in the app's languages (editor-i18n.ts); titles and
// texts in up to seven languages, the city's own (`language` of the region) required.
import { applyStatic, t } from "./editor-i18n";
import { langSwitch } from "./lang";
import { listRegions } from "./region";
import { initPlaces, loadPlaces } from "./editor-places";
import { initMottos, loadMottos } from "./editor-mottos";
import { initNotices, loadNotices } from "./editor-notices";
import { initMedia, loadMedia } from "./editor-media";
import { resizePlacesMap } from "./editor-places";
import { ask, clean, errorOf, fmtDate, fromLocal, LANGS, langName, mayLeave, setupTabs, syncTabs, toast, toLocal, watchForm } from "./editor-ui";
import type { Lang } from "./lang";
import { authHeaders, getAccess, regionsWith, roleIn, type Access } from "./opsauth";

const TOKEN_KEY = "elements.ops"; // shared with the dashboard
const GOALS = ["claims", "fronts", "finds", "points", "area", "hold"] as const;
const goalName = (g: string) => ((GOALS as readonly string[]).includes(g) ? t(`goal.n.${g as (typeof GOALS)[number]}`) : g);
const num = (n: number) => n.toLocaleString(document.documentElement.lang || "de");

type Names = Record<string, { title: string; text: string }>;
/** The story of a chapter per language: the intro, an ending per outcome. */
type Parts = { intro?: string; fire?: string; water?: string; tie?: string };
type Story = Record<string, Parts>;
const PARTS = ["intro", "fire", "water", "tie"] as const;
const STORY_MAX = 1500;
type Chapter = {
  key: string;
  campaign: string;
  names: Names;
  starts: number;
  ends: number;
  goal: string;
  target: number;
  status: "draft" | "published";
  by: string | null;
  progress: [number, number];
  story: Story | null;
};

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => $<HTMLInputElement>(id);

function storedToken(): string {
  try {
    return sessionStorage.getItem(TOKEN_KEY) ?? localStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}
function storeToken(t: string) {
  try {
    if (t) localStorage.setItem(TOKEN_KEY, t);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // storage blocked: this page only
  }
}

let token = storedToken();
let region = "";
const REGION_KEY = "ed-region";
/** The chosen city, kept across a reload: in the URL and on this device. */
function remember(slug: string) {
  try {
    localStorage.setItem(REGION_KEY, slug);
  } catch {
    // private window: the URL still carries it
  }
  const url = new URL(location.href);
  url.searchParams.set("region", slug);
  history.replaceState(history.state, "", url);
}
let chapters: Chapter[] = [];
let editing: Chapter | null = null;
let names: Names = {};
let story: Story = {};
let lang: Lang = "de";
/** The city's language: required, first tab, source of the translation aid. */
let primary: Lang = "de";
let langOf = new Map<string, string>();
const order = () => [primary, ...LANGS.filter((l) => l !== primary)];
let admin = false;
/** What the token or login opens (for the admin role per city). */
let acc: Access | null = null;

const api = (path: string, init: RequestInit = {}) =>
  fetch(path, { ...init, headers: { ...(init.headers ?? {}), ...authHeaders(token), "Content-Type": "application/json" } });

const fmt = (s: number) => fmtDate(s);

/** Without a token, a login through the proxy counts (if the server has them). */
async function access(): Promise<{ all: boolean; regions: string[] } | null> {
  const state = $("token-state");
  const { a } = await getAccess(token);
  acc = a;
  const login = !token && a?.user;
  // A login replaces the token field; "Abmelden" then goes to the login's logout.
  $("token-field").hidden = !!login;
  $("logout").hidden = login ? !a.logout : !token;
  if (!a) {
    state.textContent = token ? t("access.unknown") : "";
    return null;
  }
  const edit = regionsWith(a, "editor");
  const who = login ? `${a.user} · ` : "";
  if (!(edit === "all" || edit.length)) {
    state.textContent = login ? t("access.none", { user: a.user! }) : t("access.read");
    return null;
  }
  state.textContent = who + t("access.role", { role: t(a.role === "admin" ? "role.admin" : "role.editor") });
  return edit === "all" ? { all: true, regions: [] } : { all: false, regions: edit };
}

async function regions() {
  const a = await access();
  const sel = $<HTMLSelectElement>("region");
  if (!a) {
    sel.replaceChildren();
    $("list").textContent = t("access.login");
    $("new").hidden = true;
    for (const id of ["mottos-card", "notices-card", "places-card", "media-card"]) $(id).hidden = true;
    syncTabs();
    return;
  }
  const all = (await listRegions().catch(() => [])).filter((r) => !("url" in r));
  const mine = a.all ? all : all.filter((r) => a.regions.includes(r.slug));
  sel.replaceChildren(...mine.map((r) => new Option(r.city, r.slug)));
  langOf = new Map(mine.map((r) => [r.slug, r.language]));
  const want = new URLSearchParams(location.search).get("region") ?? localStorage.getItem(REGION_KEY);
  if (want && mine.some((r) => r.slug === want)) sel.value = want;
  region = sel.value;
  remember(region);
  admin = !!acc && roleIn(acc, region) === "admin";
  $("new").hidden = !region;
  document.querySelector(".city")!.textContent = mine.find((r) => r.slug === region)?.city ?? "";
  await load();
}

async function load() {
  if (!region) return;
  const l = langOf.get(region);
  primary = LANGS.find((x) => x === l) ?? "de";
  const res = await api(`/api/ops/chapters?region=${encodeURIComponent(region)}`).catch(() => null);
  if (!res?.ok) {
    $("list").textContent = t(res?.status === 401 ? "ch.denied" : "ch.loadfail");
    return;
  }
  chapters = await res.json();
  drawList();
  await loadMottos(region, admin, primary);
  await loadNotices(region, primary);
  await loadPlaces(region, primary);
  await loadMedia(region);
  syncTabs();
}

function state(c: Chapter): [string, string] {
  const now = Date.now() / 1000;
  // A draft due within a week doesn't play: say so loudly.
  if (c.status === "draft" && c.ends > now && c.starts - now < 7 * 86_400) return [t("ch.draft.soon"), "warn"];
  if (c.status === "draft") return [t("st.draft"), ""];
  if (now < c.starts) return [t("st.planned"), ""];
  if (now < c.ends) return [t("st.live"), "live"];
  return [t("st.over"), ""];
}

function drawList() {
  const el = $("list");
  if (!chapters.length) {
    el.className = "empty";
    el.textContent = t("ch.none");
    return;
  }
  el.className = "";
  const tbl = document.createElement("table");
  tbl.className = "ed-table";
  head(tbl, ["ch.th.chapter", "ch.th.period", "ch.th.goal", "ch.th.score", "ch.th.status"]);
  for (const c of chapters) {
    const tr = tbl.insertRow();
    const [st, cls] = state(c);
    // Text only (titles come from the city): a main line and a small one.
    const cell = (main: string, small = "", pill = "") => {
      const td = tr.insertCell();
      if (pill) {
        const sp = document.createElement("span");
        sp.className = `st ${pill === "live" ? "live" : pill === "warn" ? "warn" : ""}`;
        sp.textContent = main;
        td.append(sp);
      } else td.append(main);
      if (small) {
        const sm = document.createElement("small");
        sm.textContent = small;
        td.append(document.createElement("br"), sm);
      }
    };
    cell(c.names[primary]?.title ?? c.names.de?.title ?? c.key, c.campaign);
    cell(fmt(c.starts), t("ch.until", { date: fmt(c.ends) }));
    cell(`${num(c.target)} ${goalName(c.goal)}`);
    cell(`${num(c.progress[0])} : ${num(c.progress[1])}`);
    cell(st, c.by ?? "", cls || "pill");
    const td = tr.insertCell();
    const edit = document.createElement("button");
    edit.textContent = t("ui.edit");
    edit.onclick = async () => {
      if (await mayLeave("form")) open(c);
    };
    td.append(edit);
    if (c.status === "draft" || Date.now() / 1000 < c.starts) {
      const del = document.createElement("button");
      del.textContent = t("ui.delete");
      del.onclick = async () => {
        const name = c.names[primary]?.title ?? c.campaign;
        if (!(await ask(t("ch.delete.ask", { name }), t("ui.delete"), true))) return;
        const res = await api(`/api/ops/chapters?region=${encodeURIComponent(region)}&key=${encodeURIComponent(c.key)}`, { method: "DELETE" });
        if (!res.ok) toast(await errorOf(res, t("ui.deletefail")), "error");
        else toast(t("ch.deleted", { name }));
        await load();
      };
      td.append(del);
    }
  }
  el.replaceChildren(tbl);
}

/** A header row from keys (and an empty last column for the buttons). */
function head(tbl: HTMLTableElement, keys: Parameters<typeof t>[0][]) {
  const tr = tbl.createTHead().insertRow();
  for (const k of [...keys.map((x) => t(x)), ""]) tr.append(Object.assign(document.createElement("th"), { textContent: k }));
}

function drawLangs() {
  const tabs = $("langs");
  tabs.replaceChildren(
    ...order().map((l) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = l === lang ? "on" : "";
      const done = names[l]?.title && names[l]?.text;
      const told = PARTS.some((p) => story[l]?.[p]);
      b.innerHTML = `${langName(l)}${l === primary ? " *" : ""}<span class="lang-count">${done ? "✓" : ""}${told ? "📖" : ""}</span>`;
      b.onclick = () => {
        lang = l;
        drawLangs();
      };
      return b;
    }),
  );
  const f = $("lang-fields");
  const cur = names[lang] ?? { title: "", text: "" };
  f.innerHTML = "";
  const tl = document.createElement("label");
  tl.textContent = t("ch.f.title", { lang: langName(lang) });
  const ti = document.createElement("input");
  ti.maxLength = 60;
  ti.value = cur.title;
  tl.append(ti);
  const xl = document.createElement("label");
  xl.textContent = t("ch.f.text", { lang: langName(lang) });
  const xi = document.createElement("textarea");
  xi.maxLength = 240;
  xi.value = cur.text;
  xl.append(xi);
  const sync = () => {
    names[lang] = { title: ti.value, text: xi.value };
    preview();
    const told = PARTS.some((p) => story[lang]?.[p]);
    tabs.querySelectorAll(".lang-count")[order().indexOf(lang)].textContent = (ti.value && xi.value ? "✓" : "") + (told ? "📖" : "");
  };
  ti.oninput = sync;
  xi.oninput = sync;
  // The story (optional): the intro, an ending per outcome; paragraphs allowed.
  const box = document.createElement("details");
  box.className = "motto-langs story-box";
  box.open = PARTS.some((p) => story[lang]?.[p]);
  const sum = document.createElement("summary");
  sum.textContent = t("ch.story.sum", { lang: langName(lang) });
  const note = document.createElement("p");
  note.className = "caption";
  note.textContent = t("ch.story.note");
  box.append(sum, note);
  for (const p of PARTS) {
    const l = document.createElement("label");
    l.textContent = t(`ch.f.story.${p}`);
    const ta = document.createElement("textarea");
    ta.maxLength = STORY_MAX;
    ta.rows = 5;
    ta.value = story[lang]?.[p] ?? "";
    ta.oninput = () => {
      story[lang] = { ...story[lang], [p]: ta.value };
      sync();
    };
    l.append(ta);
    box.append(l);
  }
  const tr = document.createElement("p");
  tr.className = "caption";
  if (lang === primary) {
    tr.textContent = t("ch.primary.note", { lang: langName(primary) });
    f.append(tl, xl, tr, box);
    return;
  }
  // Translation aid: open a free web translator with the German text in a new
  // tab; the result is pasted back here. Our server sends nothing anywhere.
  tr.textContent = t("ch.tr.note", { lang: langName(primary) });
  const row = document.createElement("div");
  row.className = "ed-row";
  const de = names[primary] ?? { title: "", text: "" };
  const src = `${de.title}\n\n${de.text}`;
  const links: [string, string][] = [
    ["DeepL", `https://www.deepl.com/translator#${primary}/${lang}/${encodeURIComponent(src)}`],
    [t("ch.tr.google"), `https://translate.google.com/?sl=${primary}&tl=${lang}&op=translate&text=${encodeURIComponent(src)}`],
  ];
  for (const [label, href] of links) {
    const a = document.createElement("a");
    a.href = href;
    a.target = "_blank";
    a.rel = "noopener noreferrer";
    a.className = "ed-link";
    a.textContent = t("ch.tr.with", { service: label });
    if (!de.title || !de.text) a.setAttribute("aria-disabled", "true");
    row.append(a);
  }
  // Paste both at once: first line the title, the rest the text.
  const paste = document.createElement("button");
  paste.type = "button";
  paste.textContent = t("ch.paste");
  paste.onclick = async () => {
    const clip = (await navigator.clipboard.readText().catch(() => "")).trim();
    if (!clip) return;
    const [first, ...rest] = clip.split(/\r?\n/);
    ti.value = first.trim().slice(0, 60);
    xi.value = rest.join(" ").replace(/\s+/g, " ").trim().slice(0, 240);
    sync();
  };
  row.append(paste);
  f.append(tl, xl, tr, row, box);
}

function preview() {
  const n = names[primary] ?? { title: "", text: "" };
  $("pv-title").textContent = n.title || t("ch.pv.title");
  $("pv-text").textContent = n.text || t("ch.pv.text");
  const target = Number(input("target").value) || 0;
  $("pv-goal").textContent = t("ch.pv.goal", { n: num(target), goal: goalName(input("goal").value) });
  // A new campaign, or the first/last chapter of one: its start or end resets.
  const camp = input("campaign").value;
  const same = chapters.filter((c) => c.campaign === camp && c.key !== editing?.key && c.status === "published");
  $("reset-note").hidden = same.length > 0 && !!camp && same.some((c) => c.starts < fromLocal(input("starts").value)) && same.some((c) => c.ends > fromLocal(input("ends").value));
}

function open(c: Chapter | null) {
  clean("form");
  editing = c;
  names = c ? JSON.parse(JSON.stringify(c.names)) : {};
  story = c?.story ? JSON.parse(JSON.stringify(c.story)) : {};
  lang = primary;
  $("form-title").textContent = c ? t("ch.edit.title", { name: c.names[primary]?.title ?? c.campaign }) : t("ch.new");
  input("campaign").value = c?.campaign ?? chapters[0]?.campaign ?? "";
  const now = Math.ceil(Date.now() / 3_600_000) * 3600 + 86_400;
  input("starts").value = toLocal(c?.starts ?? now);
  input("ends").value = toLocal(c?.ends ?? now + 6 * 3600);
  input("goal").value = c?.goal ?? "claims";
  input("target").value = String(c?.target ?? 300);
  $("publish").textContent = t(c?.status === "published" ? "ui.save" : "ch.publish");
  $("save-draft").hidden = c?.status === "published";
  $("form-error").textContent = "";
  $("suggest-note").textContent = "";
  $("form-card").hidden = false;
  drawLangs();
  preview();
  $("form-card").scrollIntoView({ behavior: "smooth" });
}

async function save(publish: boolean) {
  const body = {
    key: editing?.key,
    campaign: input("campaign").value.trim(),
    names,
    starts: fromLocal(input("starts").value),
    ends: fromLocal(input("ends").value),
    goal: input("goal").value,
    target: Number(input("target").value),
    publish,
    story,
  };
  const res = await api(`/api/ops/chapters?region=${encodeURIComponent(region)}`, { method: "POST", body: JSON.stringify(body) }).catch(() => null);
  if (!res?.ok) {
    $("form-error").textContent = await errorOf(res, t("ui.savefail"));
    return;
  }
  clean("form");
  toast(t(publish ? (editing?.status === "published" ? "ch.saved" : "ch.published") : "ch.drafted"));
  $("form-card").hidden = true;
  await load();
}

async function suggest() {
  const q = new URLSearchParams({ region, goal: input("goal").value, starts: String(fromLocal(input("starts").value)), ends: String(fromLocal(input("ends").value)) });
  const res = await api(`/api/ops/chapters/suggest?${q}`).catch(() => null);
  const want = res?.ok ? ((await res.json()) as { target: number | null }).target : null;
  if (want) {
    input("target").value = String(want);
    $("suggest-note").textContent = t("ch.sugg.ok");
  } else {
    $("suggest-note").textContent = t("ch.sugg.none");
  }
  preview();
}

applyStatic();
langSwitch();
input("token").value = token;
input("token").onchange = () => {
  token = input("token").value.trim();
  storeToken(token);
  void regions();
};
$("logout").onclick = async () => {
  if (!(await mayLeave())) return;
  if (!token && acc?.logout) return void location.assign(acc.logout);
  token = "";
  input("token").value = "";
  storeToken("");
  void regions();
};
$<HTMLSelectElement>("region").onchange = async () => {
  const sel = $<HTMLSelectElement>("region");
  if (!(await mayLeave())) {
    sel.value = region;
    return;
  }
  region = sel.value;
  remember(region);
  admin = !!acc && roleIn(acc, region) === "admin";
  document.querySelector(".city")!.textContent = sel.selectedOptions[0]?.text ?? "";
  $("form-card").hidden = true;
  void load();
};
$("new").onclick = async () => {
  if (await mayLeave("form")) open(null);
};
setupTabs({ orte: () => resizePlacesMap() });
watchForm("form");
initPlaces(api);
initMottos(api);
initNotices(api);
initMedia(api);
$("cancel").onclick = async () => {
  if (!(await mayLeave("form"))) return;
  $("form-card").hidden = true;
};
$("save-draft").onclick = () => void save(false);
$("publish").onclick = () => void save(true);
$("suggest").onclick = () => void suggest();
for (const id of ["target", "goal", "campaign", "starts", "ends"]) input(id).addEventListener("input", preview);
void regions();
