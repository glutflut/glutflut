// Player progress (web.rs `Score`, `Ability`, `Challenge`): name and level in
// the header, the abilities row (level 2-5) and the personal challenge card.
import { t, type Key } from "./i18n";
import type { AbilityName, ChallengeKind, Faction, Mine, Net, ServerMsg } from "./net";
import type { Sound } from "./sound";
import { CLAIM_RADIUS_M } from "./geo";
import { $, mmss, toast } from "./page";
import { KEYS, load, save } from "./store";
import { figure, mediaFor } from "./media";

type Msg<K extends ServerMsg["t"]> = Extract<ServerMsg, { t: K }>;

export function setupProgress(net: Net, sound: Sound, faction: () => Faction | null, onRadius: () => void) {
  let mine: Mine | null = null;
  let lastLevel = 0;
  let lastStreak: Mine["streak"] = null;
  const sideName = () => t(faction() === "fire" ? "side.fire" : "side.water");
  const levelName = (n: number) => t(`level.${faction()}.${Math.min(5, Math.max(1, n))}` as Key);
  function showMine() {
    if (!mine || !faction()) return;
    $("me-label").textContent = mine.name;
    // With XP (rule sets with progression) the level name stands for the side.
    const who = mine.xp > 0 ? levelName(mine.level) : sideName();
    $("me-sub").textContent = t("points.today", { side: who, n: mine.today });
    // Test servers: the level picker shows the level.
    ($("level-test-pick") as HTMLSelectElement).value = String(mine.level);
    // The level on the side's dot in the header.
    const dot = $("me-dot");
    dot.textContent = mine.xp > 0 ? String(mine.level) : "";
    dot.classList.toggle("lvl", mine.xp > 0);
    dot.title = mine.xp > 0 ? t(mine.next ? "level.progress" : "level.max", { n: mine.level, name: levelName(mine.level), xp: mine.xp, next: mine.next ?? "" }) : "";
    $("my-level").textContent =
      mine.xp > 0
        ? t(mine.next ? "level.progress" : "level.max", { n: mine.level, name: levelName(mine.level), xp: mine.xp, next: mine.next ?? "" })
        : "";
    if (lastLevel && mine.level > lastLevel) {
      toast(t("level.up", { name: levelName(mine.level) }));
      sound.motif();
    }
    lastLevel = mine.level;
    showStreak(mine.streak);
    drawAbilities();
  }

  // Weekly streak (score.rs `StreakView`): a line under the level, a toast
  // for each new play day and for a finished week.
  function showStreak(s: Mine["streak"]) {
    const el = $("my-streak");
    el.hidden = !s;
    if (!s) return;
    const icon = faction() === "fire" ? "🔥" : "💧";
    const dots = "●".repeat(Math.min(s.days, s.need)) + "○".repeat(Math.max(0, s.need - s.days));
    const week = t("streak.week", { dots, need: s.need });
    const head = !s.counts
      ? t("streak.pause", { icon, n: s.weeks })
      : s.weeks === 1
        ? t("streak.one", { icon })
        : s.weeks > 1
          ? t("streak.many", { icon, n: s.weeks })
          : t("streak.zero");
    el.textContent = `${head} · ${week}`;
    el.title = t("streak.help", { need: s.need, spare: s.spare });
    const before = lastStreak;
    lastStreak = s;
    if (!before) return;
    if (s.weeks > before.weeks) {
      toast(t("streak.done", { icon, n: s.weeks }));
      sound.motif();
    } else if (s.days > before.days) {
      toast(t("streak.day", { n: s.days, need: s.need }));
    }
  }

  // Abilities (level 2-5): a row of buttons; Weitblick and Funkenflug/Welle
  // are armed for the next claim, the others act at once. Cooldowns here.
  const ABILITY_LEVEL: Record<AbilityName, number> = { reach: 2, nourish: 3, boost: 4, double: 5 };
  let abilitiesOn = false;
  let reachM = CLAIM_RADIUS_M;
  /** Supply on (rule set): the anchor gap in m, else null. */
  let supply: number | null = null;
  /** Reinforcing on (rule set): own ground at the front takes a plain source. */
  let reinforce = false;
  const ab: Record<AbilityName, { armed: boolean; until: number }> = {
    reach: { armed: false, until: 0 },
    nourish: { armed: false, until: 0 },
    boost: { armed: false, until: 0 },
    double: { armed: false, until: 0 },
  };
  const abLabel = (n: AbilityName) => t(n === "double" ? (`ab.double.${faction()}` as Key) : (`ab.${n}` as Key));
  const claimRadius = () => (ab.reach.armed ? reachM : CLAIM_RADIUS_M);
  function drawAbilities() {
    const el = $("abilities");
    const level = mine?.level ?? 1;
    const open = (Object.keys(ABILITY_LEVEL) as AbilityName[]).filter((n) => level >= ABILITY_LEVEL[n]);
    el.hidden = !abilitiesOn || !open.length;
    if (el.hidden) return;
    const now = Date.now();
    el.replaceChildren(
      ...open.map((n) => {
        const left = Math.ceil((ab[n].until - now) / 1000);
        const b = Object.assign(document.createElement("button"), {
          type: "button",
          textContent: left > 0 ? `${abLabel(n)} · ${mmss(left)}` : abLabel(n),
          disabled: left > 0,
        });
        b.classList.toggle("armed", ab[n].armed);
        b.setAttribute("aria-pressed", String(ab[n].armed));
        b.onclick = () => net.send({ t: "ability", name: n });
        return b;
      }),
    );
  }
  setInterval(() => abilitiesOn && drawAbilities(), 1000);

  // Personal challenge: card above the buttons, countdown kept here.
  type Ch = { kind: ChallengeKind; goal: number; progress: number; until: number; done: boolean; hint?: string; xp: number; wait: boolean };
  let ch: Ch | null = null;
  // Entdecken: the zone's image, if the city gave it one (once per hint).
  let mediaHint = "";
  function drawZoneImage(hint: string) {
    if (hint === mediaHint) return;
    mediaHint = hint;
    const slot = $("challenge").querySelector(".ch-media")!;
    slot.replaceChildren();
    if (hint)
      void mediaFor("zone", hint).then((m) => {
        if (m && hint === mediaHint) slot.append(figure(m));
      });
  }
  function drawChallenge() {
    const el = $("challenge");
    drawZoneImage(ch && !ch.done && !ch.wait ? (ch.hint ?? "") : "");
    if (!ch) return void (el.hidden = true);
    el.hidden = false;
    el.classList.toggle("done", ch.done || ch.wait);
    const left = Math.max(0, (ch.until - Date.now()) / 1000);
    // Pause between challenges: only the countdown.
    if (ch.wait) {
      el.querySelector(".ch-time")!.textContent = t("ch.next", { t: mmss(left) });
      el.querySelector(".ch-count")!.textContent = "…";
      el.querySelector(".ch-text")!.textContent = t("ch.wait");
      el.querySelector(".ch-hint")!.textContent = "";
      (el.querySelector(".ch-bar span") as HTMLElement).style.width = "0%";
      return;
    }
    el.querySelector(".ch-time")!.textContent = t(ch.done ? "ch.next" : "ch.left", { t: mmss(left) });
    el.querySelector(".ch-count")!.textContent = ch.done ? "✓" : `${Math.min(ch.progress, ch.goal)}/${ch.goal}`;
    el.querySelector(".ch-text")!.textContent = (ch.done ? "✓ " : "") + t(`ch.${ch.kind}` as Key, { goal: ch.goal });
    el.querySelector(".ch-hint")!.textContent = ch.done
      ? t("ch.done", { xp: ch.xp })
      : ch.hint
        ? t("ch.discover.hint", { hint: ch.hint })
        : "";
    (el.querySelector(".ch-bar span") as HTMLElement).style.width = `${(100 * Math.min(ch.progress, ch.goal)) / ch.goal}%`;
  }
  setInterval(drawChallenge, 1000);
  // Folded to one line (kept per device); a tap on it unfolds.
  const foldChallenge = (on: boolean) => {
    $("challenge").classList.toggle("folded", on);
    save(KEYS.challengeFolded, on ? "1" : "0");
  };
  $("challenge").classList.toggle("folded", load(KEYS.challengeFolded) === "1");
  $("ch-fold").onclick = (e) => {
    e.stopPropagation();
    foldChallenge(true);
  };
  $("challenge").onclick = () => $("challenge").classList.contains("folded") && foldChallenge(false);

  return {
    mine: () => mine,
    show: showMine,
    setMine(m: Mine) {
      mine = m;
      showMine();
    },
    claimRadius,
    supply: () => supply,
    reinforce: () => reinforce,
    welcome(m: Msg<"welcome">) {
      abilitiesOn = !!m.abilities;
      reachM = m.reach_m ?? CLAIM_RADIUS_M;
      supply = m.supply ? (m.anchor_gap_m ?? 200) : null;
      reinforce = m.reinforce ?? false;
    },
    onAbility(m: Msg<"ability">) {
      ab[m.name] = { armed: m.armed, until: Date.now() + m.ready_in * 1000 };
      if (!m.ok) {
        const why = m.reason === "nothing" ? (m.name === "boost" ? "ab.err.boost" : "ab.err.nourish") : m.reason === "level" ? "ab.err.level" : "ab.err.cooldown";
        toast(t(why as Key));
      } else if (m.armed) {
        toast(t("ab.armed", { name: abLabel(m.name) }));
      } else if (m.name === "nourish" || m.name === "boost") {
        toast(t(`ab.${m.name}.ok` as Key));
      }
      onRadius();
      drawAbilities();
    },
    onChallenge(m: Msg<"challenge">) {
      ch = { kind: m.kind, goal: m.goal, progress: m.progress, until: Date.now() + m.ends_in * 1000, done: m.done, hint: m.hint, xp: ch?.xp ?? 0, wait: !!m.wait };
      drawChallenge();
    },
    onChallengeDone(m: Msg<"challengedone">) {
      if (ch) ch.xp = m.xp;
      toast(t("ch.done", { xp: m.xp }));
      drawChallenge();
    },
    onChallengePaused() {
      ch = null;
      drawChallenge();
    },
  };
}
