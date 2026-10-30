// Everything the app keeps in this browser (localStorage), in one place; the
// privacy page (datenschutz.html) describes it. Reads and writes never throw:
// with storage blocked or full, values live in memory for this visit.

export const KEYS = {
  /** Player token from `welcome` (net.ts), mirrored to the cookie. */
  token: "elements.token",
  /** First claim on this device got the big burst (burst.ts). */
  firstBurst: "elements.burst.first",
  lang: "elements.lang",
  region: "elements.region",
  intro: "elements.intro",
  /** Tapped "Mitspielen" (watch.ts); before that a first visit only watches. */
  joined: "elements.joined",
  /** Last "what's new" entry shown (news.ts). */
  newsSeen: "elements.news.seen",
  /** Age band and "local" answers, only from 16 (profile.ts). */
  profile: "elements.profile",
  adult: "elements.adult",
  /** Objection to the city statistics (consent.ts). */
  stats: "elements.stats",
  findsAsked: "elements.finds.asked",
  /** Pending presence code (presence.ts), at most 10 minutes. */
  presence: "elements.presence",
  /** Chapter titles from the server, by key. */
  chapters: "elements.chapters",
  /** Story cards shown on this device (chapter intros and endings, story.ts). */
  storySeen: "elements.story.seen",
  /** News flashes shown on this device (notices.ts). */
  noticesSeen: "elements.notices.seen",
  chapterFolded: "elements.chapter.folded",
  challengeFolded: "elements.challenge.folded",
  topFolded: "elements.top.folded",
  gfx: "elements.gfx",
  gfxAuto: "elements.gfx.auto",
  /** Earlier switches, read once for `gfx`. */
  oldView: "elements.view",
  oldAnim: "elements.anim",
  sound: "elements.sound",
  soundAmbient: "elements.sound.ambient",
  ar: "elements.ar",
  dashboardData: "elements.dashboard.data",
  /** Install hint (install.ts): first day seen, hidden until. */
  install: "elements.install",
  /** Rain radar layer on (radar.ts). */
  radar: "elements.radar",
  /** Cloud cover layer on (radar.ts). */
  clouds: "elements.clouds",
  /** Day of the last invite nudge (invite.ts). */
  inviteNudge: "elements.invite.nudge",
  /** Keys to others' permanent sources (gift.ts). */
  gifts: "elements.gifts",
} as const;

/** A fixed key, or one per region (permanent source ticket) or per place (wish sent). */
export type StoreKey = (typeof KEYS)[keyof typeof KEYS] | `elements.perm.${string}` | `elements.wish.${string}`;

const mem = new Map<string, string>();

/** Storage can't be used at all (blocked): values last this visit only. */
export const blocked = (() => {
  try {
    localStorage.getItem(KEYS.token);
    return false;
  } catch {
    return true;
  }
})();

export function load(key: StoreKey): string | null {
  try {
    return mem.get(key) ?? localStorage.getItem(key);
  } catch {
    return mem.get(key) ?? null;
  }
}

/** `null` removes the key. */
export function save(key: StoreKey, value: string | null) {
  mem.delete(key);
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    if (value !== null) mem.set(key, value);
  }
}

/** Parsed JSON, or `null` if missing or broken. */
export function loadJson<T>(key: StoreKey): T | null {
  try {
    return JSON.parse(load(key) ?? "null") as T | null;
  } catch {
    return null;
  }
}

export function saveJson(key: StoreKey, value: unknown) {
  save(key, value === null ? null : JSON.stringify(value));
}
