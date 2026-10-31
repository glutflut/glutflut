//! The WebSocket JSON protocol: what the app sends (`ClientMsg`) and gets
//! (`ServerMsg`). `web/src/protocol.ts` is generated from these types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::hotspot::HotspotInfo;
use crate::score::Mine;
use crate::tiles::TileKey;
use crate::{campaign, challenge, finds, info, weather};

/// What the server sends.
#[derive(Serialize, TS)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ServerMsg<'a> {
    /// Answer to a presence code: confirmed (`n` codes in all) or why not
    /// (`far`, `stale`, `flagged`; `known` counts as ok; `unknown` comes
    /// from the gateway, without `n`).
    Presence {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        n: Option<usize>,
    },
    /// Answer to `ping`, from the gateway.
    Pong,
    /// Too many new players from this client: the app backs off.
    Busy,
    /// A recovery code that failed (`unknown`, or `wait` when rate-limited);
    /// success is `recovered`.
    Recover {
        ok: bool,
        #[ts(type = "\"unknown\" | \"wait\"")]
        reason: &'static str,
    },
    /// Solve this before a new identity is made (pow.rs).
    Pow {
        c: &'a str,
        bits: u32,
    },
    Welcome {
        token: &'a str,
        #[ts(as = "Faction")]
        faction: &'static str,
        /// Tap-to-move (`?sim`) allowed.
        sim: bool,
        /// Levels and abilities on (rule set), and Weitblick's reach.
        levels: bool,
        abilities: bool,
        reach_m: f32,
        /// Supply on: a claim on own ground places an anchor, at least this far from the next.
        supply: bool,
        anchor_gap_m: f32,
        /// Reinforcing on: where no anchor fits, a claim on own ground at the
        /// front places an ordinary source.
        reinforce: bool,
        /// Permanent sources on; this player's slots in use here (by tickets
        /// the app may hold), how many at level 1 and how many more per
        /// level; their minimum distance.
        keep: bool,
        keep_used: u32,
        keep_base: u32,
        keep_per_level: u32,
        keep_gap_m: f32,
        /// Permanent sources can be linked (`[keep] link`).
        keep_link: bool,
        /// Tag lines for permanent sources: all, with what unlocks them.
        mottos: Vec<MottoItem>,
        /// Hotspot bonus radius (`hotspot_radius_m`), for the "you're here" hint.
        hotspot_m: f32,
        /// Build of the server: an app of another one reloads (web/src/update.ts).
        version: String,
        /// This player's invite code (empty for watchers), and the points an
        /// invite brings once the friend really plays.
        invite: String,
        invite_points: i64,
        /// A watcher (hello with `watch`): no token, no side, no score.
        watch: bool,
    },
    State {
        nodes: Vec<(i64, f32, f32)>,
    },
    /// Territory per tile, all (`full`) or the tiles that changed.
    Overview {
        tiles: Vec<[u32; 6]>,
        full: bool,
    },
    /// Nodes per side in the whole region (the app only holds some tiles).
    Totals {
        fire: u32,
        water: u32,
        front: u32,
    },
    /// Connections cheering per side (last 5 min), and the new cheers
    /// [fire, water] since the last message (`cheer.rs`; for show only).
    Cheers {
        fire: u32,
        water: u32,
        burst: [u32; 2],
    },
    Claim {
        node: i64,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(
            type = "\"range\" | \"cooldown\" | \"unknown\" | \"moving\" | \"own\" | \"strong\" | \"supplied\" | \"worn\" | \"linked\" | \"linkfar\" | \"linkdown\" | \"nokey\"",
            optional
        )]
        reason: Option<&'static str>,
        bonus: bool,
        points: i64,
        /// Seconds until the next claim is possible (cooldown or motion lock),
        /// so the app can show the wait on the button.
        wait: f32,
        /// Seconds until the claim is at full strength (on enemy ground).
        build: f32,
        /// Put out a spark / placed an anchor.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        doused: bool,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        anchor: bool,
        /// Through a link into another region: where it landed there (lon, lat).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        far: Option<[f64; 2]>,
        /// With `far`: the junctions around it there as (lon, lat, fire,
        /// water), for the app's small map of that place.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        near: Option<Vec<[f64; 4]>>,
        /// With `near`: the streets between those junctions (index pairs).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        streets: Option<Vec<[u16; 2]>>,
        /// With `far` (or a link here): the permanent source it reached
        /// from there (lon, lat).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        from: Option<[f64; 2]>,
    },
    Rate {
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<&'static str>,
        points: i64,
    },
    Hotspots {
        items: Vec<HotspotInfo>,
    },
    Score(Mine),
    /// The finds this player knows here, and the size of each collection.
    Finds {
        collect: bool,
        items: Vec<&'a finds::Find>,
        #[ts(type = "Record<string, number>")]
        totals: HashMap<String, u32>,
    },
    /// The region's news flashes running now (all of them, each time).
    Notices {
        items: &'a [campaign::Notice],
    },
    /// The city's questions running now (all of them, each time).
    Questions {
        items: &'a [campaign::Question],
    },
    /// Uncovered by a claim: `n` of `of` in its collection; `stored` only
    /// when collecting.
    Find {
        #[serde(flatten)]
        find: &'a finds::Find,
        n: u32,
        of: u32,
        stored: bool,
    },
    /// Something of collection `c` within `[finds] hint_m` (`first_hint_m`
    /// before the first find): `m` metres away, rounded to 10.
    Near {
        c: &'a str,
        m: u32,
    },
    /// A tunnel opened: claim `node` from afar within `secs` (see `Tunnels`).
    Tunnel {
        node: i64,
        lat: f64,
        lon: f64,
        /// "peak" (a fire hilltop) or "sink" (a water sink).
        #[ts(type = "\"peak\" | \"sink\"")]
        kind: &'static str,
        m: u32,
        secs: u32,
    },
    /// Just earned (the app names it).
    Badge {
        badge: &'a str,
    },
    /// A ring (held block) broken: the points this player got for it.
    RingBroken {
        points: i64,
    },
    /// Invites of this player that counted: the points paid now.
    Invited {
        points: i64,
    },
    /// Every minute (blocks on): held blocks with players' sources on the
    /// border, and those among them with this player's.
    Areas {
        player: Vec<i32>,
        mine: Vec<i32>,
    },
    /// This player's sources closed a ring around a source of `side` (`siege.rs`).
    Encircled {
        points: i64,
        #[ts(as = "Faction")]
        side: &'static str,
    },
    /// The current or next campaign chapter and the race.
    Chapter {
        #[serde(flatten)]
        chapter: &'a campaign::Chapter,
        starts_in: f64,
        ends_in: f64,
        fire: i64,
        water: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(as = "Option<Faction>", optional)]
        winner: Option<&'static str>,
        /// The first chapter of a campaign where levels are on: everyone
        /// starts again at level 1 (`Scores::campaign_reset`).
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        #[ts(as = "Option<bool>", optional)]
        resets: bool,
    },
    Forgotten,
    /// Today's weather as a rule (`[weather] enabled`).
    Weather(weather::Shown),
    /// The tide at the waterfront (`tide.rs`), in regions with a gauge.
    Tide(crate::tide::Shown),
    /// Side switched (test servers).
    Side {
        #[ts(as = "Faction")]
        faction: &'static str,
    },
    /// A fresh recovery code (shown behind a tap in the app).
    Code {
        code: &'a str,
    },
    /// Code redeemed: the app stores this token and reconnects.
    Recovered {
        token: &'a str,
    },
    /// The current challenge (new or changed).
    Challenge(challenge::View),
    /// Just finished, with the XP it brought.
    ChallengeDone {
        kind: elements_shared::progression::Challenge,
        xp: u32,
    },
    /// A round without claims: no new challenge until the next claim.
    ChallengePaused,
    /// Answer to an ability: used (or armed for the next claim), or why not.
    Ability {
        #[ts(as = "AbilityName")]
        name: &'static str,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(type = "\"level\" | \"cooldown\" | \"nothing\"", optional)]
        reason: Option<&'static str>,
        armed: bool,
        ready_in: u32,
    },
    /// Claimed sources: [node, faction, seconds left, lifetime, yours, lon, lat,
    /// build-up seconds left, build-up in total, kind] (build-up 0 once full;
    /// kind "spark", "anchor", "cut" (no supply) or ""). Who placed a source
    /// is only told to that player.
    Sources {
        #[ts(as = "Vec<SourceRow>")]
        items: Vec<SourceItem>,
    },
    /// Changes to `sources` since the last message: `set` adds or replaces
    /// (same rows), `gone` removes. Apps that load by tiles get only their
    /// tiles, and the sources of newly held tiles as `set`.
    Sourcesdiff {
        #[ts(as = "Vec<SourceRow>")]
        set: Vec<SourceItem>,
        gone: Vec<i64>,
    },
    /// Answer to `keep`: placed (with the new ticket, shown to nobody
    /// else), topped up, or why not (`off`, `moving`, `range`, `unknown`,
    /// `notclaim`, `near`, `slot`, `notyours`, `cooldown`).
    Keep {
        node: i64,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        ticket: Option<String>,
        topup: bool,
        wait: f32,
    },
    /// Answer to `redeem`: the ticket's source `standing` (at `node`),
    /// `fallen` or `gone`, and the points paid now; `ticket` as sent (the
    /// app holds several).
    Kept {
        ticket: String,
        #[ts(type = "\"standing\" | \"fallen\" | \"gone\"")]
        state: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        node: Option<i64>,
        points: i64,
    },
    /// Slot freed without a ticket (or not: once a day).
    Released {
        ok: bool,
    },
    /// Answer to `motto`: set, or why not (`off`, `notyours`, `locked`).
    Motto {
        node: i64,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<&'static str>,
    },
    /// Answer to `peek`: the junction a claim through the link of `via`
    /// would take now, with its position; none when nothing is to take there.
    Peek {
        via: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        node: Option<i64>,
        /// No target: why (`linked` nothing to take, `nokey`, `linkfar`,
        /// `linkdown`; `asleep`: the far region sleeps, a claim wakes it).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(
            type = "\"linked\" | \"nokey\" | \"linkfar\" | \"linkdown\" | \"asleep\"",
            optional
        )]
        reason: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        lon: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        lat: Option<f64>,
    },
    /// A found key (Funkenflug `ember`, Flaschenpost `bottle`) to a permanent
    /// source of the own side in region `region` (its `name`).
    Found {
        #[ts(type = "\"ember\" | \"bottle\"")]
        origin: &'static str,
        region: String,
        name: String,
        key: String,
    },
    /// Answer to `gift`: a key to the source for someone else (`key`, shown
    /// once), or why not (`off`, `notyours`, `soon`).
    Gift {
        node: i64,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(type = "\"off\" | \"notyours\" | \"soon\"", optional)]
        reason: Option<&'static str>,
    },
    /// Answer to `link`: linked or unlinked, or why not (`off`, `notyours`, `same`).
    Link {
        node: i64,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<&'static str>,
    },
    /// Card of a tapped junction (`info.rs`). `take`: ok, own (your side's
    /// source), mine (your ground), anchor (your ground, places one), strong
    /// (`wait` s until weak enough, if within two days), range (`dist` m),
    /// watch (a watcher: no side).
    Info {
        node: i64,
        fire: f32,
        water: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        source: Option<info::Src>,
        #[ts(
            type = "\"ok\" | \"own\" | \"mine\" | \"anchor\" | \"strong\" | \"range\" | \"watch\""
        )]
        take: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        wait: Option<u32>,
        dist: u32,
        /// How far a claim reaches now (more with Weitblick armed).
        reach: u32,
        elev: f32,
        #[ts(type = "\"peak\" | \"sink\" | \"slope\" | \"flat\"")]
        terrain: &'static str,
        dh: f32,
        #[ts(type = "\"\" | Faction")]
        lean: &'static str,
        /// Rain here now from the radar (mm/h, 0 dry; `radar.rs`) and the
        /// frame's time (UTC); absent without radar data.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        rain: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        rain_at: Option<String>,
    },
    /// `Rules::rise`: at a fitting place, stay `secs` (`wait`); the source
    /// rose (`done`); the player left too early (`left`); or the other side
    /// grew too strong there meanwhile (`strong`).
    Rise {
        node: i64,
        lat: f64,
        lon: f64,
        #[ts(type = "\"wait\" | \"done\" | \"left\" | \"strong\"")]
        state: &'static str,
        secs: u32,
        /// `done`: junctions taken (the spot and its hollow / hilltop).
        took: u32,
    },
    /// A spark of the other element appeared `m` metres away.
    Spark {
        node: i64,
        lat: f64,
        lon: f64,
        m: u32,
    },
    /// To everyone here: a claim reached through a link from the permanent
    /// source at `from` to `to` (lon, lat; which source A it came from stays
    /// unsaid).
    Beam {
        from: [f64; 2],
        to: [f64; 2],
        #[ts(as = "Faction")]
        side: &'static str,
    },
}

impl ServerMsg<'_> {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ServerMsg serializes")
    }
}

/// Sources claimed by players (natural ones are fixed and not sent), with
/// real seconds left and in total at this node's ageing speed.
/// One entry of `sources`, sent as an array (see `ServerMsg::Sources`).
pub struct SourceItem {
    pub node: i64,
    pub faction: &'static str,
    pub left: u32,
    pub life: u32,
    pub mine: bool,
    pub lon: f64,
    pub lat: f64,
    pub build: u32,
    pub ramp: u32,
    pub kind: &'static str,
    /// A claim that can't become a permanent source (natural source near).
    pub no_keep: bool,
    /// A permanent source's tag line (`mottos` key, `c-<n>` for the city's).
    pub motto: Option<std::borrow::Cow<'static, str>>,
    /// Inside a ring of the other side's players (`siege.rs`): ages faster.
    pub sieged: bool,
    /// A permanent source linked with another of its owner's (not which).
    pub linked: bool,
}

impl Serialize for SourceItem {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let i = self;
        (
            i.node, i.faction, i.left, i.life, i.mine, i.lon, i.lat, i.build, i.ramp, i.kind,
            i.no_keep, &i.motto, i.sieged, i.linked,
        )
            .serialize(s)
    }
}

/// A tag line (`elements_shared::mottos`): its key, side, the level it
/// needs and a badge it needs (prefix), if any. The city's campaign phrases
/// (`c-<n>`) bring their texts.
#[derive(Serialize, TS)]
pub struct MottoItem {
    #[ts(as = "String")]
    pub key: std::borrow::Cow<'static, str>,
    #[ts(as = "Faction")]
    pub side: &'static str,
    pub level: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub badge: Option<&'static str>,
    /// City phrases: `{lang: text}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "Record<string, string>")]
    pub names: Option<serde_json::Value>,
}

impl MottoItem {
    /// The built-in phrases, then the running campaign's (one entry per side).
    pub fn all(city: &[crate::campaign::CityMotto]) -> Vec<Self> {
        let builtin = elements_shared::mottos::MOTTOS.iter().map(|m| Self {
            key: m.key.into(),
            side: m.side.as_str(),
            level: m.level,
            badge: m.badge,
            names: None,
        });
        let city = city.iter().flat_map(|c| {
            elements_shared::prelude::Faction::ALL
                .into_iter()
                .filter(|f| c.side.is_none_or(|s| s == *f))
                .map(|f| Self {
                    key: format!("c-{}", c.num).into(),
                    side: f.as_str(),
                    level: 1,
                    badge: None,
                    names: Some(c.names.clone()),
                })
        });
        builtin.chain(city).collect()
    }
}

/// What `SourceItem` serializes to (TS only).
#[derive(TS)]
#[allow(dead_code)]
struct SourceRow(
    i64,
    Faction,
    u32,
    u32,
    bool,
    f64,
    f64,
    u32,
    u32,
    SourceKind,
    bool,
    Option<String>,
    bool,
    bool,
);

// String sets the server sends as `&'static str`, for TS only.

/// `Faction::as_str`.
#[derive(TS)]
#[ts(rename_all = "lowercase")]
#[allow(dead_code)]
pub(crate) enum Faction {
    Fire,
    Water,
}

/// Abilities by name (`web/handlers/ability.rs`).
#[derive(TS)]
#[ts(rename_all = "lowercase")]
#[allow(dead_code)]
pub(crate) enum AbilityName {
    Reach,
    Nourish,
    Boost,
    Double,
}

/// `web::source_kind`; "" is a plain claim.
#[derive(TS)]
#[ts(rename_all = "lowercase")]
#[allow(dead_code)]
pub(crate) enum SourceKind {
    #[ts(rename = "")]
    Claim,
    Spark,
    Anchor,
    Cut,
    Keep,
}

/// Age group in `profile`.
#[derive(TS)]
#[allow(dead_code)]
pub(crate) enum Age {
    #[ts(rename = "u30")]
    U30,
    #[ts(rename = "30_59")]
    Mid,
    #[ts(rename = "60p")]
    Old,
}

#[derive(Deserialize, TS)]
pub struct PowSolution {
    pub c: String,
    pub n: String,
}

/// The far end of a link in another region: the region and a key to the
/// permanent source there (its ticket); the server passes the key on, never stores it.
#[derive(Deserialize, TS, Clone, Debug)]
pub struct Far {
    pub region: String,
    pub key: String,
}

/// What the app sends.
#[derive(Deserialize, TS)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientMsg {
    Hello {
        token: Option<String>,
        #[serde(default)]
        #[ts(as = "Option<bool>", optional)]
        bot: bool,
        #[serde(default)]
        #[ts(as = "Option<bool>", optional)]
        tiles: bool,
        /// From another shard (see `shards.rs`).
        #[serde(default)]
        #[ts(optional = nullable)]
        passport: Option<String>,
        /// A solved proof-of-work challenge (`pow.rs`), for a new identity.
        #[serde(default)]
        #[ts(optional = nullable)]
        pow: Option<PowSolution>,
        /// Only watch: no player, no score, nothing counted; the gateway
        /// passes on only what watching needs (view, overview, node info).
        #[serde(default)]
        #[ts(as = "Option<bool>", optional)]
        watch: bool,
        /// The invite code from the link a new player came by.
        #[serde(default)]
        #[ts(optional = nullable)]
        invite: Option<String>,
    },
    /// The tiles the app holds now (replaces the last view).
    View {
        tiles: Vec<TileKey>,
    },
    /// Zoomed out: send territory per tile (and its changes) while on.
    Overview {
        on: bool,
    },
    Pos {
        lat: f64,
        lon: f64,
        /// Device speed in m/s (Geolocation `coords.speed`), when reported.
        #[serde(default)]
        #[ts(optional = nullable)]
        spd: Option<f32>,
        /// Accuracy in m (`coords.accuracy`).
        #[serde(default)]
        #[ts(optional = nullable)]
        acc: Option<f32>,
    },
    /// `via`: the ticket of an own linked permanent source in reach, for a
    /// claim around its partner (`[keep] link`).
    Claim {
        node: i64,
        #[serde(default)]
        #[ts(optional = nullable)]
        via: Option<String>,
        /// The link of `via` goes into another region.
        #[serde(default)]
        #[ts(optional = nullable)]
        far: Option<Far>,
    },
    /// Make an own claim permanent, or top up a permanent source (with its ticket).
    Keep {
        node: i64,
        #[serde(default)]
        #[ts(optional = nullable)]
        ticket: Option<String>,
    },
    /// Points of a permanent source (standing or fallen) for its ticket.
    Redeem {
        ticket: String,
    },
    /// Free the slots of permanent sources this app has no ticket for
    /// (`tickets`: the ones it has).
    Release {
        #[serde(default)]
        tickets: Vec<String>,
    },
    /// A tapped junction: strength, source, takeable, terrain.
    Info {
        node: i64,
    },
    /// One card: `cat`, and more answers of the same card in `more`.
    Rate {
        node: i64,
        cat: String,
        #[serde(default)]
        #[ts(optional)]
        more: Option<Vec<String>>,
    },
    Rename,
    Hide {
        on: bool,
    },
    Forget,
    Profile {
        #[serde(default)]
        #[ts(as = "Option<Age>", optional = nullable)]
        age: Option<String>,
        #[serde(default)]
        #[ts(optional = nullable)]
        local: Option<bool>,
    },
    /// Make a new recovery code.
    Code,
    /// Liveness check; answered with `pong` right here (phones drop sockets
    /// silently while asleep).
    Ping,
    /// Use an ability (reach, nourish, boost, double).
    Ability {
        #[ts(as = "AbilityName")]
        name: String,
    },
    /// Redeem a recovery code on this device.
    Recover {
        code: String,
    },
    /// Switch sides ("fire" / "water"); test servers (`ELEMENTS_ALLOW_SIM`) only.
    Side {
        #[ts(as = "Faction")]
        faction: String,
    },
    /// Cheer for a side (players only their own; for show only, `cheer.rs`).
    Cheer {
        #[ts(as = "Faction")]
        side: String,
    },
    /// Test servers only: set this player's level (1-5) for trying abilities.
    Level {
        level: u8,
    },
    /// Give an own permanent source a tag line (`mottos` key; none: remove).
    Motto {
        node: i64,
        ticket: String,
        #[serde(default)]
        #[ts(optional = nullable)]
        motto: Option<String>,
    },
    /// What a claim through the link of the own source with ticket `via`
    /// would take now (the app asks while standing at it).
    Peek {
        via: String,
        #[serde(default)]
        #[ts(optional = nullable)]
        far: Option<Far>,
    },
    /// A key to the own permanent source at `node` (its ticket) for someone else.
    Gift {
        node: i64,
        ticket: String,
    },
    /// Link the own permanent source at `node` with the one at `to` (both
    /// tickets), or unlink it (no `to`).
    Link {
        node: i64,
        ticket: String,
        #[serde(default)]
        #[ts(optional = nullable)]
        to: Option<i64>,
        #[serde(default)]
        #[ts(optional = nullable)]
        to_ticket: Option<String>,
    },
    /// Keep the finds one uncovers (opt-in), or stop and forget them.
    Collect {
        on: bool,
    },
    /// Count in the analytics (default) or not.
    Stats {
        on: bool,
    },
    /// A presence code from the game link (`?p=`).
    Presence {
        code: String,
    },
}

/// `web/src/protocol.ts`: the messages and every named type they use.
#[cfg(test)]
fn typescript() -> String {
    use ts_rs::Config;
    let cfg = Config::new().with_large_int("number");
    macro_rules! decls {
        ($($t:ty),* $(,)?) => {
            vec![$((<$t as TS>::ident(&cfg), <$t as TS>::docs(), <$t as TS>::decl(&cfg))),*]
        };
    }
    let decls = decls![
        ServerMsg<'static>,
        ClientMsg,
        Far,
        Faction,
        AbilityName,
        SourceKind,
        SourceRow,
        MottoItem,
        Age,
        PowSolution,
        Mine,
        crate::score::StreakView,
        finds::Find,
        HotspotInfo,
        campaign::Goal,
        campaign::Notice,
        campaign::Question,
        weather::Shown,
        crate::tide::Shown,
        challenge::View,
        info::Src,
        elements_shared::progression::Challenge,
    ];
    let mut out = String::from(
        "// Generated from crates/server/src/protocol.rs by ts-rs; do not edit.\n\
         // Regenerate: UPDATE_PROTOCOL=1 cargo test -p elements-server protocol\n",
    );
    for (_, docs, decl) in decls {
        out.push('\n');
        if let Some(d) = docs {
            out.push_str(&d);
        }
        // Field docs stay in Rust; one variant per line in the unions.
        let mut decl = decl;
        while let Some(i) = decl.find("\n/**") {
            let j = i + decl[i..].find("*/\n").expect("closed doc") + 3;
            decl.replace_range(i..j, "");
        }
        let decl = decl.replace(" | { \"t\"", "\n  | { \"t\"");
        out.push_str(&format!("export {decl}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_item_is_an_array() {
        let i = SourceItem {
            node: 7,
            faction: "fire",
            left: 1,
            life: 2,
            mine: true,
            lon: 9.5,
            lat: 47.5,
            build: 3,
            ramp: 4,
            kind: "keep",
            no_keep: false,
            motto: Some("f-glows-on".into()),
            sieged: false,
            linked: true,
        };
        assert_eq!(
            serde_json::to_string(&i).unwrap(),
            r#"[7,"fire",1,2,true,9.5,47.5,3,4,"keep",false,"f-glows-on",false,true]"#
        );
    }

    /// Exact frames as the gateway sent them as raw strings (cached apps
    /// match on them).
    #[test]
    fn gateway_frames_are_unchanged() {
        assert_eq!(ServerMsg::Pong.to_json(), r#"{"t":"pong"}"#);
        assert_eq!(ServerMsg::Busy.to_json(), r#"{"t":"busy"}"#);
        for reason in ["unknown", "wait"] {
            assert_eq!(
                ServerMsg::Recover { ok: false, reason }.to_json(),
                format!(r#"{{"t":"recover","ok":false,"reason":"{reason}"}}"#)
            );
        }
        let refused = ServerMsg::Presence {
            ok: false,
            reason: Some("unknown"),
            n: None,
        };
        assert_eq!(
            refused.to_json(),
            r#"{"t":"presence","ok":false,"reason":"unknown"}"#
        );
        let ok = ServerMsg::Presence {
            ok: true,
            reason: None,
            n: Some(3),
        };
        assert_eq!(ok.to_json(), r#"{"t":"presence","ok":true,"n":3}"#);
    }

    #[test]
    fn sourcesdiff_frame() {
        let m = ServerMsg::Sourcesdiff {
            set: Vec::new(),
            gone: vec![5],
        };
        assert_eq!(m.to_json(), r#"{"t":"sourcesdiff","set":[],"gone":[5]}"#);
    }

    const PROTOCOL_TS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/src/protocol.ts");

    /// Fails when the Rust messages changed and protocol.ts wasn't regenerated.
    #[test]
    fn protocol_ts_is_current() {
        let ts = typescript();
        if std::env::var_os("UPDATE_PROTOCOL").is_some() {
            std::fs::write(PROTOCOL_TS, &ts).unwrap();
            return;
        }
        let old = std::fs::read_to_string(PROTOCOL_TS).unwrap_or_default();
        assert!(
            old == ts,
            "web/src/protocol.ts is stale; run UPDATE_PROTOCOL=1 cargo test -p elements-server protocol"
        );
    }

    /// Every named type the messages use is declared in protocol.ts.
    #[test]
    fn protocol_ts_declares_all_dependencies() {
        struct Deps<'a>(&'a ts_rs::Config, std::collections::BTreeSet<String>);
        impl ts_rs::TypeVisitor for Deps<'_> {
            fn visit<T: TS + 'static + ?Sized>(&mut self) {
                if T::output_path().is_some() && self.1.insert(T::ident(self.0)) {
                    T::visit_dependencies(self);
                }
            }
        }
        let cfg = ts_rs::Config::new();
        let mut deps = Deps(&cfg, Default::default());
        ServerMsg::visit_dependencies(&mut deps);
        ClientMsg::visit_dependencies(&mut deps);
        let ts = typescript();
        for name in deps.1 {
            assert!(
                ts.contains(&format!("export type {name} ")),
                "{name} missing in typescript()"
            );
        }
    }

    #[test]
    fn ts_factions_match() {
        let names: Vec<_> = elements_shared::prelude::Faction::ALL
            .iter()
            .map(|f| format!("\"{}\"", f.as_str()))
            .collect();
        assert_eq!(Faction::inline(&ts_rs::Config::new()), names.join(" | "));
    }
}
