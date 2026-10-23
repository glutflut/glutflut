// Generated from crates/server/src/protocol.rs by ts-rs; do not edit.
// Regenerate: UPDATE_PROTOCOL=1 cargo test -p elements-server protocol

/**
 * What the server sends.
 */
export type ServerMsg = { "t": "presence", ok: boolean, reason?: string, n?: number, }
  | { "t": "pong" }
  | { "t": "busy" }
  | { "t": "recover", ok: boolean, reason: "unknown" | "wait", }
  | { "t": "pow", c: string, bits: number, }
  | { "t": "welcome", token: string, faction: Faction, sim: boolean, levels: boolean, abilities: boolean, reach_m: number, supply: boolean, anchor_gap_m: number, reinforce: boolean, keep: boolean, keep_used: number, keep_base: number, keep_per_level: number, keep_gap_m: number, mottos: Array<MottoItem>, hotspot_m: number, version: string, watch: boolean, }
  | { "t": "state", nodes: Array<[number, number, number]>, }
  | { "t": "overview", tiles: Array<[number, number, number, number, number, number]>, full: boolean, }
  | { "t": "totals", fire: number, water: number, front: number, }
  | { "t": "cheers", fire: number, water: number, burst: [number, number], }
  | { "t": "claim", node: number, ok: boolean, reason?: "range" | "cooldown" | "unknown" | "moving" | "own" | "strong" | "supplied" | "worn", bonus: boolean, points: number, wait: number, build: number, doused?: boolean, anchor?: boolean, }
  | { "t": "rate", ok: boolean, reason?: string, points: number, }
  | { "t": "hotspots", items: Array<HotspotInfo>, }
  | { "t": "score" } & Mine
  | { "t": "finds", collect: boolean, items: Array<FindItem>, totals: Record<string, number>, }
  | { "t": "notices", items: Array<Notice>, }
  | { "t": "find", n: number, of: number, stored: boolean, id: number, c: string, name: string, lat: number, lon: number, text?: Record<string, string>, }
  | { "t": "near", c: string, }
  | { "t": "tunnel", node: number, lat: number, lon: number, kind: "peak" | "sink", m: number, secs: number, }
  | { "t": "badge", badge: string, }
  | { "t": "ringbroken", points: number, }
  | { "t": "areas", player: Array<number>, mine: Array<number>, }
  | { "t": "encircled", points: number, side: Faction, }
  | { "t": "chapter", starts_in: number, ends_in: number, fire: number, water: number, winner?: Faction, key: string, campaign: string, names: Record<string, { title: string; text: string }>, goal: ChapterGoal, target: number, target_water: number, kind?: string, nodes?: number[], at?: Array<[number, number]>, story?: Record<string, { intro?: string; fire?: string; water?: string; tie?: string }>, }
  | { "t": "forgotten" }
  | { "t": "weather" } & WeatherShown
  | { "t": "tide" } & TideShown
  | { "t": "side", faction: Faction, }
  | { "t": "code", code: string, }
  | { "t": "recovered", token: string, }
  | { "t": "challenge" } & ChallengeView
  | { "t": "challengedone", kind: ChallengeKind, xp: number, }
  | { "t": "challengepaused" }
  | { "t": "ability", name: AbilityName, ok: boolean, reason?: "level" | "cooldown" | "nothing", armed: boolean, ready_in: number, }
  | { "t": "sources", items: Array<SourceRow>, }
  | { "t": "sourcesdiff", set: Array<SourceRow>, gone: Array<number>, }
  | { "t": "keep", node: number, ok: boolean, reason?: string, ticket?: string, topup: boolean, wait: number, }
  | { "t": "kept", ticket: string, state: "standing" | "fallen" | "gone", node?: number, points: number, }
  | { "t": "released", ok: boolean, }
  | { "t": "motto", node: number, ok: boolean, reason?: string, }
  | { "t": "info", node: number, fire: number, water: number, source?: NodeSource, take: "ok" | "own" | "mine" | "anchor" | "strong" | "range" | "watch", wait?: number, dist: number, reach: number, elev: number, terrain: "peak" | "sink" | "slope" | "flat", dh: number, lean: "" | Faction, rain?: number, rain_at?: string, }
  | { "t": "spark", node: number, lat: number, lon: number, m: number, };

/**
 * What the app sends.
 */
export type ClientMsg = { "t": "hello", token: string | null, bot?: boolean, tiles?: boolean, passport?: string | null, pow?: PowSolution | null, watch?: boolean, }
  | { "t": "view", tiles: Array<[number, number]>, }
  | { "t": "overview", on: boolean, }
  | { "t": "pos", lat: number, lon: number, spd?: number | null, acc?: number | null, }
  | { "t": "claim", node: number, }
  | { "t": "keep", node: number, ticket?: string | null, }
  | { "t": "redeem", ticket: string, }
  | { "t": "release", tickets: Array<string>, }
  | { "t": "info", node: number, }
  | { "t": "rate", node: number, cat: string, }
  | { "t": "rename" }
  | { "t": "hide", on: boolean, }
  | { "t": "forget" }
  | { "t": "profile", age?: Age | null, local?: boolean | null, }
  | { "t": "code" }
  | { "t": "ping" }
  | { "t": "ability", name: AbilityName, }
  | { "t": "recover", code: string, }
  | { "t": "side", faction: Faction, }
  | { "t": "cheer", side: Faction, }
  | { "t": "level", level: number, }
  | { "t": "motto", node: number, ticket: string, motto?: string | null, }
  | { "t": "collect", on: boolean, }
  | { "t": "stats", on: boolean, }
  | { "t": "presence", code: string, };

/**
 * `Faction::as_str`.
 */
export type Faction = "fire" | "water";

/**
 * Abilities by name (`web/handlers/ability.rs`).
 */
export type AbilityName = "reach" | "nourish" | "boost" | "double";

/**
 * `web::source_kind`; "" is a plain claim.
 */
export type SourceKind = "" | "spark" | "anchor" | "cut" | "keep";

/**
 * What `SourceItem` serializes to (TS only).
 */
export type SourceRow = [number, Faction, number, number, boolean, number, number, number, number, SourceKind, boolean, string | null, boolean];

/**
 * A tag line (`elements_shared::mottos`): its key, side, the level it
 * needs and a badge it needs (prefix), if any. The city's campaign phrases
 * (`c-<n>`) bring their texts.
 */
export type MottoItem = { key: string, side: Faction, level: number, badge?: string, names?: Record<string, string>, };

/**
 * Age group in `profile`.
 */
export type Age = "u30" | "30_59" | "60p";

export type PowSolution = { c: string, n: string, };

/**
 * What a player sees about their own standing.
 */
export type Mine = { name: string, xp: number, level: number, next: number | null, today: number, total: number, rank_today: number | null, rank_total: number | null, hidden: boolean, badges: Array<string>, title: boolean, streak: StreakView | null, };

/**
 * The streak as the app shows it.
 */
export type StreakView = { weeks: number, days: number, need: number, spare: number, counts: boolean, };

export type FindItem = { id: number, c: string, name: string, lat: number, lon: number, text?: Record<string, string>, };

export type HotspotInfo = { node: number, ends_in: number, manual: boolean, ll: [number, number], };

export type ChapterGoal = "claims" | "fronts" | "finds" | "points" | "area" | "hold";

/**
 * A news flash of the region (`game.notices`), shown while it runs.
 */
export type Notice = { id: number, names: Record<string, { title: string; text: string }>, url?: string, };

/**
 * What the app shows: today's main effect (`None`: calm) and the day.
 */
export type WeatherShown = { effect: "rain" | "heat" | "frost" | null, side: Faction | null, level: number, tmax: number, tmin: number, precip_mm: number, };

/**
 * What the app shows (`ServerMsg::Tide`): off, or the phase and its direction.
 */
export type TideShown = { on: boolean, phase: number, rising: boolean, level_m: number, next_high: number | null, next_low: number | null, hilo_estimated: boolean, };

/**
 * What the player sees.
 */
export type ChallengeView = { kind: ChallengeKind, goal: number, progress: number, ends_in: number, done: boolean, hint?: string, wait?: boolean, };

/**
 * The source on a junction as the card shows it.
 */
export type NodeSource = { kind: "natural" | "claim" | "anchor" | "spark" | "keep", faction: Faction, left?: number, gone?: number, build: number, yours: boolean, };

/**
 * Challenges the game proposes (no choice), unlocked by level.
 */
export type ChallengeKind = "claim3" | "discover" | "front" | "quiet" | "hold" | "retake" | "team";
