// Leaderboard sheet (server: score.rs, `/api/leaderboard`) with two panes:
// ranking (faction duel, top 10, own progress), profile (name, privacy
// switches, app settings) and help (how to play, news, feedback).
import { t } from "./i18n";
import type { Faction, Mine, Net } from "./net";
import { badgeLabel } from "./badges";
import { isAdult, isCounted, onAdult, setAdult, setCounted } from "./consent";
import { clearProfile } from "./profile";
import { withRegion } from "./region";
import { COLORS } from "./geo";
import { $ } from "./page";
import { ask } from "./ask";

interface Entry {
  name: string;
  faction: Faction;
  points: number;
  title?: boolean;
  /** Weeks in a row (streak). */
  streak?: number;
}
interface Side {
  node_minutes: number;
  points: number;
  wins: number;
}
interface Board {
  today: Entry[];
  season: Entry[];
  /** Since the campaign start. */
  campaign?: Entry[];
  fire: Side;
  water: Side;
  /** Weekly title holders (fire, water). */
  titles?: [string | null, string | null];
  /** City streak: days in a row with `need` people (today included once reached). */
  city?: { days: number; today: boolean; missing: number; need: number };
}

export function setupBoard(net: Net) {
  const el = $("board");
  let data: Board | null = null;
  let mine: Mine | null = null;
  let tab: "today" | "campaign" | "season" = "today";
  let pane: "rank" | "me" | "help" = "rank";
  let timer = 0;

  const esc = (s: string) => s.replace(/[&<>"]/g, (c) => `&#${c.charCodeAt(0)};`);
  function render() {
    if (mine) {
      $("my-name").textContent = mine.name;
      ($("listed") as HTMLInputElement).checked = !mine.hidden;
      const rank = tab === "today" ? mine.rank_today : tab === "campaign" ? mine.rank_campaign : mine.rank_total;
      const pts = tab === "today" ? mine.today : tab === "campaign" ? mine.campaign : mine.total;
      $("my-rank").textContent = mine.hidden ? t("board.hidden", { pts }) : rank ? t("board.rank", { rank, pts }) : t("board.points", { pts });
      const badges = $("my-badges");
      badges.replaceChildren(
        ...(mine.title ? [t("board.title.mine")] : []).concat((mine.badges ?? []).map(badgeLabel)).map((b, i) => {
          const chip = document.createElement("span");
          chip.className = i === 0 && mine?.title ? "chip title" : "chip";
          chip.textContent = b;
          return chip;
        }),
      );
    }
    if (!data) return;
    const f = data.fire.node_minutes, w = data.water.node_minutes;
    const share = f + w > 0 ? f / (f + w) : 0.5;
    $("duel-fire").style.width = `${share * 100}%`;
    $("duel-water").style.width = `${(1 - share) * 100}%`;
    $("duel-note").textContent = t("board.duel.note");
    $("duel-fire-n").textContent = t("board.duel.fire", { share: Math.round(share * 100), pts: data.fire.points });
    $("duel-water-n").textContent = t("board.duel.water", { share: Math.round((1 - share) * 100), pts: data.water.points });
    $("season").textContent = t("board.wins", { fire: data.fire.wins, water: data.water.wins });
    const [tf, tw] = data.titles ?? [null, null];
    $("titles").textContent = tf || tw ? t("board.titles", { fire: tf ?? "–", water: tw ?? "–" }) : "";
    const c = data.city;
    $("city-streak").textContent = !c
      ? ""
      : (c.days > 0 ? t(c.days === 1 ? "city.streak.one" : "city.streak", { n: c.days }) : t("city.streak.none")) +
        (c.today ? "" : ` · ${t(c.missing === 1 ? "city.missing.one" : "city.missing", { n: c.missing, day: c.days + 1 })}`);
    $("city-streak").title = c ? t("city.help", { need: c.need }) : "";
    const rows = tab === "today" ? data.today : tab === "campaign" ? (data.campaign ?? []) : data.season;
    $("ranks").innerHTML = rows.length
      ? rows
          .map(
            (r) =>
              `<li class="${mine && !mine.hidden && r.name === mine.name ? "me" : ""}"><span class="dot" style="background:${COLORS[r.faction]}"></span><span class="name">${r.title ? "★ " : ""}${esc(r.name)}${r.streak ? ` <span class="streak">${r.faction === "fire" ? "🔥" : "💧"}${r.streak}</span>` : ""}</span><span>${r.points}</span></li>`,
          )
          .join("")
      : `<li class="empty">${t(tab === "today" ? "board.empty.today" : tab === "campaign" ? "board.empty.campaign" : "board.empty.season")}</li>`;
  }
  async function load() {
    try {
      const res = await fetch(withRegion("/api/leaderboard"));
      if (res.ok) data = await res.json();
    } catch {
      // keep the last one
    }
    render();
  }
  // Poll only while the ranking is on screen.
  const poll = () => {
    clearInterval(timer);
    if (el.hidden || pane !== "rank") return;
    load();
    timer = window.setInterval(load, 10_000);
  };
  const show = (p: typeof pane) => {
    pane = p;
    for (const b of el.querySelectorAll<HTMLButtonElement>(".seg button")) {
      const on = b.dataset.pane === p;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", String(on));
    }
    for (const x of ["rank", "me", "help"]) $(`pane-${x}`).hidden = p !== x;
    el.scrollTop = 0;
    poll();
  };
  const hide = () => {
    el.hidden = true;
    clearInterval(timer);
  };
  // One history entry while open: back (button, gesture) closes.
  const open = () => {
    el.hidden = false;
    if (!history.state?.board) history.pushState({ ...history.state, board: true }, "");
    show(pane);
  };
  const close = () => {
    hide();
    if (history.state?.board) history.back();
  };
  addEventListener("popstate", () => {
    if (!history.state?.board) hide();
  });
  $("me").onclick = () => (el.hidden ? open() : close());
  $("board-close").onclick = close;
  for (const b of el.querySelectorAll<HTMLButtonElement>(".seg button")) b.onclick = () => show(b.dataset.pane as typeof pane);
  const setTab = (to: typeof tab) => {
    tab = to;
    for (const x of el.querySelectorAll<HTMLButtonElement>(".tabs button")) x.classList.toggle("on", x.dataset.tab === to);
    render();
  };
  for (const b of el.querySelectorAll<HTMLButtonElement>(".tabs button")) b.onclick = () => setTab(b.dataset.tab as typeof tab);
  $("rename").onclick = () => net.send({ t: "rename" });
  ($("listed") as HTMLInputElement).onchange = (e) =>
    net.send({ t: "hide", on: !(e.target as HTMLInputElement).checked });
  // Objection to the analytics: play on, uncounted (Art. 21 GDPR).
  const statsBox = $("stats-on") as HTMLInputElement;
  statsBox.checked = isCounted();
  statsBox.onchange = () => {
    setCounted(statsBox.checked);
    net.send({ t: "stats", on: statsBox.checked });
  };
  // 16 or older: unlocks finds and the optional questions (Art. 8 GDPR).
  const adultBox = $("adult") as HTMLInputElement;
  adultBox.checked = isAdult();
  adultBox.onchange = async () => {
    if (!adultBox.checked) {
      // Stays ticked until answered.
      adultBox.checked = true;
      if (!(await ask(t("age.off.confirm")))) return;
      adultBox.checked = false;
    }
    setAdult(adultBox.checked);
  };
  onAdult((on) => {
    adultBox.checked = on;
    if (!on) clearProfile();
  });
  $("forget").onclick = async () => {
    if (await ask(t("board.forget.confirm"))) net.send({ t: "forget" });
  };
  return {
    /** Watchers: the ranking (their name opens the cities instead). */
    openRank() {
      pane = "rank";
      open();
    },
    /** Kiosk tour: the ranking for a while, the season when nobody scored today. */
    async showRank() {
      await load();
      setTab(data && !data.today.length && data.season.length ? "season" : "today");
      el.hidden = false;
      show("rank");
    },
    hideRank: hide,
    mine(m: Mine) {
      mine = m;
      if (!el.hidden) render();
    },
  };
}
