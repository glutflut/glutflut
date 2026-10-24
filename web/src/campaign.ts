// Campaign chapter (server: campaign.rs): a bar under the header while a
// chapter runs (race between the sides) or is coming up, a card when a side
// wins. Chapter names are kept per device for badge labels. A "hold" chapter
// rings its junctions on the map from the week before until it ends.
import * as maplibregl from "maplibre-gl";
import { lang, t, type Key } from "./i18n";
import type { ChapterMsg } from "./net";
import { layoutTop } from "./top";
import { $ } from "./page";
import type { Media } from "./media";
import { withRegion } from "./region";
import { KEYS, load, loadJson, save, saveJson } from "./store";
import { markStorySeen, pickLang, showStory, storySeen } from "./story";


/** To the nearest minute: the relative seconds arrive a little late, and 17:59:58 must not read "17:59". */
const minute = (ms: number) => Math.round(ms / 60_000) * 60_000;

type Names = Record<string, Record<string, { title: string; text: string }>>;

function stored(): Names {
  return loadJson<Names>(KEYS.chapters) ?? {};
}

const WEATHER_ICON: Record<string, string> = {
  rain: "☔", frost: "❄", hot: "☀", cold: "🌡", warm: "🌡", sunny: "☀", grey: "🌫", mild: "⛅",
};

/** Title and text: a weather day's from the locales, a campaign's from the server. */
function texts(m: ChapterMsg): { title: string; text: string } {
  const w = m.kind?.startsWith("weather:") ? m.kind.slice(8) : null;
  if (w) return { title: `${WEATHER_ICON[w] ?? ""} ${t(`wx.${w}.title` as Key)}`.trim(), text: t(`wx.${w}.text` as Key) };
  return m.names[lang] ?? m.names.de ?? m.names.en ?? Object.values(m.names)[0] ?? { title: m.key, text: "" };
}

/** Title of chapter `key` in the current language (known on this device). */
export function chapterTitle(key: string): string {
  const n = stored()[key];
  return (n?.[lang] ?? n?.de ?? n?.en ?? (n ? Object.values(n)[0] : undefined))?.title ?? key;
}

/** A chapter's story parts in the app's language (or the city's). */
type StoryParts = { intro?: string; fire?: string; water?: string; tie?: string };
function storyOf(story: Record<string, StoryParts> | undefined): StoryParts {
  return pickLang(story, lang) ?? {};
}

/** The intro once when the chapter runs; the ending for the outcome once it is over. */
function tell(key: string, title: string, story: Record<string, StoryParts> | undefined, phase: "intro" | "end", winner: string | null | undefined) {
  const s = storyOf(story);
  const text = phase === "intro" ? s.intro : winner === "fire" ? s.fire : winner === "water" ? s.water : s.tie;
  const mark = `${phase}:${key}`;
  if (!text || storySeen(mark)) return;
  markStorySeen(mark);
  const kicker = phase === "intro" ? t("chapter.intro") : winner ? t(`campaign.won.${winner}` as Key) : t("chapter.end");
  showStory({
    cls: phase === "end" && winner ? winner : "badge",
    kicker,
    title,
    text,
    media: ["challenge", key],
    ...(phase === "end" ? { url: withRegion("/chronik.html"), urlLabel: t("chapter.chronicle") } : {}),
  });
}

/** A finished chapter of the chronicle (`/api/chronicle`). */
type Chron = { key: string; names: ChapterMsg["names"]; story?: Record<string, StoryParts>; ends: number; state: string; winner: string | null };

function duration(s: number): string {
  const m = Math.round(s / 60);
  if (m < 60) return t("time.min", { m });
  const h = Math.floor(m / 60);
  return h < 48 ? t("time.hmin", { h, m: m % 60 }) : t("time.days", { d: Math.round(h / 24) });
}

export function setupCampaign(
  map: maplibregl.Map,
  notice: (cls: string, kicker: string, name: string, line: string, media?: [Media["kind"], string]) => void,
) {
  let last: ChapterMsg | null = null;
  let at = 0;
  let announced = "";
  // The hold chapter's rings, for chapter `ringsOf`.
  let rings: maplibregl.Marker[] = [];
  let ringsOf = "";
  function ring(m: ChapterMsg | null) {
    const key = m?.at?.length ? m.key : "";
    if (key === ringsOf) return;
    for (const r of rings) r.remove();
    ringsOf = key;
    // The pulse on an inner element: the marker's own transform places it.
    rings = (key && m?.at ? m.at : []).map((ll) => {
      const el = document.createElement("div");
      el.className = "hold-mark";
      el.append(Object.assign(document.createElement("div"), { className: "hold-ring" }));
      return new maplibregl.Marker({ element: el }).setLngLat(ll).addTo(map);
    });
  }

  function render() {
    const bar = $("chapter");
    const hide = () => {
      bar.hidden = true;
      layoutTop();
    };
    if (!last) return hide();
    const m = last;
    const gone = (Date.now() - at) / 1000;
    const startsIn = m.starts_in - gone, endsIn = m.ends_in - gone;
    const name = texts(m);
    // Coming up: only in the last week before; over: hidden.
    const shown = endsIn > 0 && startsIn <= 7 * 86400;
    ring(shown ? m : null);
    if (!shown) return hide();
    bar.hidden = false;
    bar.classList.toggle("folded", folded() === m.key);
    // The chronicle: campaign chapters only (weather days are not in it).
    const chron = $("chapter-chron") as HTMLAnchorElement;
    chron.hidden = !!m.kind;
    chron.href = withRegion("/chronik.html");
    layoutTop();
    $("chapter-title").textContent = name?.title ?? m.key;
    if (startsIn > 0) {
      bar.classList.add("soon");
      const when = new Date(minute(Date.now() + startsIn * 1000)).toLocaleString(lang, { weekday: "short", hour: "2-digit", minute: "2-digit" });
      $("chapter-time").textContent = t("campaign.starts", { when });
      $("chapter-goal").textContent = name?.text ?? "";
      return;
    }
    bar.classList.remove("soon");
    $("chapter-time").textContent = m.winner ? t(`campaign.won.${m.winner}` as Key) : t("campaign.ends", { time: duration(endsIn) });
    const goal = t(`campaign.goal.${m.goal}` as Key);
    const end = new Date(minute(Date.now() + endsIn * 1000)).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });
    $("chapter-goal").textContent =
      m.goal === "hold"
        ? t("campaign.hold", { n: m.target, time: end })
        : m.target === m.target_water
        ? t("campaign.goal", { target: m.target.toLocaleString(lang), goal })
        : t("campaign.goal.sides", { fire: m.target.toLocaleString(lang), water: m.target_water.toLocaleString(lang), goal });
    const fill = (id: string, v: number, target: number) => (($(id) as HTMLElement).style.width = `${Math.min(100, (100 * v) / target)}%`);
    fill("chapter-fire", m.fire, m.target);
    fill("chapter-water", m.water, m.target_water);
    $("chapter-fire-n").textContent = m.fire.toLocaleString(lang);
    $("chapter-water-n").textContent = m.water.toLocaleString(lang);
  }

  // The chapter folded to one line (until the next one).
  const folded = () => load(KEYS.chapterFolded);
  const fold = (key: string | null) => {
    save(KEYS.chapterFolded, key);
    render();
  };
  $("chapter-fold").onclick = (e) => {
    e.stopPropagation();
    if (last) fold(last.key);
  };
  $("chapter").onclick = () => {
    if (!last) return;
    // Folded: a tap unfolds it; unfolded: the chapter's text as a card.
    if ($("chapter").classList.contains("folded")) return fold(null);
    const name = texts(last);
    notice("badge", t(last.kind ? "campaign.weather" : "campaign.chapter"), name.title, name.text, last.kind ? undefined : ["challenge", last.key]);
  };
  // The link in the bar: not a tap on the bar.
  $("chapter-chron").onclick = (e) => e.stopPropagation();
  for (const a of document.querySelectorAll<HTMLAnchorElement>(".foot-chron")) a.href = withRegion("/chronik.html");
  $("chapter").onkeydown = (e) => {
    if (e.key === "Enter" || e.key === " ") $("chapter").click();
  };
  setInterval(render, 30_000);

  return {
    onChapter(m: ChapterMsg) {
      if (!m.kind) {
        saveJson(KEYS.chapters, { ...stored(), [m.key]: m.names });
      }
      const before = last?.key === m.key ? last.winner : undefined;
      last = m;
      at = Date.now();
      render();
      // A side reached the target: once per chapter and device session.
      if (m.winner && !before && announced !== m.key && m.starts_in <= 0) {
        announced = m.key;
        notice(m.winner, t(m.kind ? "campaign.weather" : "campaign.chapter"), t(`campaign.won.${m.winner}` as Key), texts(m).title);
      }
      // The story: its intro while it runs, its ending once it is over.
      if (!m.kind && m.story) {
        if (m.starts_in <= 0 && m.ends_in > 0) tell(m.key, texts(m).title, m.story, "intro", null);
        if (m.ends_in <= 0) tell(m.key, texts(m).title, m.story, "end", m.winner ?? null);
      }
    },
    /** Who wasn't there at the end: the newest ending of the last two weeks, once. */
    async catchUp() {
      try {
        const res = await fetch(withRegion("/api/chronicle"));
        if (!res.ok) return;
        const data = (await res.json()) as { campaigns: { chapters: Chron[] }[] };
        const now = Date.now() / 1000;
        const over = data.campaigns
          .flatMap((c) => c.chapters)
          .filter((c) => c.state === "over" && c.story && now - c.ends < 14 * 86400)
          .sort((a, b) => b.ends - a.ends)[0];
        if (!over) return;
        const name = pickLang(over.names, lang)?.title ?? over.key;
        tell(over.key, name, over.story, "end", over.winner);
      } catch {
        // offline: next time
      }
    },
  };
}
