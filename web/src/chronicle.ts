// The chronicle (chronik.html): what happened in the region's campaigns,
// from `/api/chronicle` (server: chronicle.rs). Finished chapters with their
// winner and the story's ending, the running one with its progress and
// intro, the next ones with their dates only (no spoilers).
import { lang, loadCity, t, type Key } from "./i18n";
import { figure, mediaFor } from "./media";
import { withRegion } from "./region";
import { paragraphs, pickLang } from "./story";

type Parts = { intro?: string; fire?: string; water?: string; tie?: string };
type Chapter = {
  key: string;
  names: Record<string, { title: string; text: string }>;
  story: Record<string, Parts> | null;
  starts: number;
  ends: number;
  goal: string;
  target: number;
  target_water: number;
  fire: number;
  water: number;
  state: "over" | "live" | "soon";
  winner: "fire" | "water" | null;
};
type Data = { city: string | null; campaigns: { campaign: string; chapters: Chapter[] }[] };

const $ = (id: string) => document.getElementById(id)!;
const num = (n: number) => n.toLocaleString(lang);
const day = (s: number) => new Date(s * 1000).toLocaleDateString(lang, { weekday: "short", day: "numeric", month: "long" });
const time = (s: number) => new Date(s * 1000).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });

/** "advent-2026" as a heading: words, capitalised. */
function campaignName(slug: string): string {
  return slug
    .split("-")
    .map((w) => (/^\d+$/.test(w) ? w : w.charAt(0).toUpperCase() + w.slice(1)))
    .join(" ");
}

function chapter(c: Chapter): HTMLElement {
  const art = document.createElement("article");
  art.className = `chap ${c.state === "over" ? (c.winner ?? "tie") : c.state}`;
  const names = pickLang(c.names, lang);
  const h = document.createElement("h3");
  h.textContent = names?.title ?? c.key;
  const when = document.createElement("p");
  when.className = "when";
  when.textContent = `${day(c.starts)}, ${time(c.starts)} – ${day(c.ends)}, ${time(c.ends)}`;
  art.append(h, when);
  const story = pickLang(c.story, lang) ?? {};
  if (c.state === "soon") {
    const p = document.createElement("p");
    p.textContent = t("chron.soon", { when: `${day(c.starts)}, ${time(c.starts)}` });
    art.append(p);
    return art;
  }
  if (names?.text) {
    const p = document.createElement("p");
    p.textContent = names.text;
    art.append(p);
  }
  // The race.
  const race = document.createElement("div");
  race.className = "race";
  const bars = document.createElement("div");
  bars.className = "bars";
  const f = document.createElement("span");
  f.className = "f";
  f.style.width = `${Math.min(50, (50 * c.fire) / Math.max(1, c.target))}%`;
  const w = document.createElement("span");
  w.className = "w";
  w.style.width = `${Math.min(50, (50 * c.water) / Math.max(1, c.target_water))}%`;
  bars.append(f, w);
  race.append(`🔥 ${num(c.fire)}`, bars, `${num(c.water)} 💧`);
  const goal = document.createElement("p");
  goal.className = "when";
  const goalName = t(`campaign.goal.${c.goal}` as Key);
  goal.textContent =
    c.goal === "hold"
      ? t("campaign.hold", { n: c.target, time: time(c.ends) })
      : c.target === c.target_water
        ? t("campaign.goal", { target: num(c.target), goal: goalName })
        : t("campaign.goal.sides", { fire: num(c.target), water: num(c.target_water), goal: goalName });
  art.append(race, goal);
  const slot = document.createElement("div");
  art.append(slot);
  void mediaFor("challenge", c.key).then((m) => m && slot.append(figure(m)));
  const text = document.createElement("div");
  text.className = "story";
  if (c.state === "live") {
    const p = document.createElement("p");
    p.className = "outcome";
    p.textContent = t("chron.live");
    art.append(p);
    if (story.intro) text.append(...paragraphs(story.intro));
  } else {
    const p = document.createElement("p");
    p.className = "outcome";
    p.textContent = c.winner ? t(`chron.won.${c.winner}` as Key) : t("chron.tie");
    art.append(p);
    const ending = c.winner ? story[c.winner] : story.tie;
    if (ending) text.append(...paragraphs(ending));
  }
  art.append(text);
  return art;
}

function render(data: Data) {
  const host = $("chron");
  host.replaceChildren();
  const campaigns = data.campaigns.filter((c) => c.chapters.length);
  if (!campaigns.length) {
    const p = document.createElement("p");
    p.textContent = t("chron.none");
    host.append(p);
    return;
  }
  for (const c of campaigns) {
    const sec = document.createElement("section");
    sec.className = "chron-campaign";
    const h = document.createElement("h2");
    h.textContent = campaignName(c.campaign);
    sec.append(h, ...[...c.chapters].reverse().map(chapter));
    host.append(sec);
  }
}

function labels() {
  $("chron-back").textContent = t("chron.back");
  $("chron-title").textContent = t("chron.title");
  $("chron-lead").textContent = t("chron.lead");
  document.title = `${t("chron.title")} – Glut & Flut`;
}

labels();
loadCity(labels);
fetch(withRegion("/api/chronicle"))
  .then((r) => (r.ok ? (r.json() as Promise<Data>) : Promise.reject(new Error(String(r.status)))))
  .then(render)
  .catch(() => {
    $("chron").textContent = t("chron.none");
  });
