# Changelog

All notable changes to Glut & Flut. Versions follow
[Semantic Versioning](https://semver.org/); the format is loosely
[Keep a Changelog](https://keepachangelog.com/).

## Unreleased

### Added

- Side reveal card: the first time a browser plays, a card says which side
  it got, what to do first ("Erobern", "Nächste Front") and that the side
  stays. Once per device; the side is only a bias towards the weaker one, so
  the card does not claim that.
- Time to the first claim (migration V5): `analytics.daily` counts per day
  how many new identities joined and how many of them claimed within five
  minutes; the dashboard shows the share as "Erste fünf Minuten".
- The first tip: with nothing to take in range, the claim button itself
  points to the nearest front ("Nächste Front: 40 m ↗"); a tap with no
  junction in reach says so instead of "alles deins".
- The day's first challenge at level 1 is always "claim 3 junctions".
- Before the first find, the hint reaches `[finds] first_hint_m` (400 m)
  instead of 150 m, and every hint says how far ("noch etwa 120 m").
- Linked permanent sources (`[keep] link`, off by default; `rules/link.toml`
  for a playtest; migration V6): a player links two of their own Glut/Eis in
  the player sheet; standing at one, the claim button reaches the junctions
  around the other (both ways, any distance) at `link_factor` of the
  strength, never the partner itself. Others see a dotted ring on a linked
  source, not its partner; the owner sees their links as dashed lines on the
  map and links or unlinks from the junction card of an own source too. At a
  linked source a second button "Drüben erobern" appears when something is
  to take around the partner (the app asks the server, `peek`); after the
  claim the camera shows the spot for a moment and comes back.

## 0.2.0 – 2026-10-24: story campaigns and the chronicle

### Added

- Story-driven campaigns (migration V3): a chapter may carry a story (an
  intro shown once when it starts, an ending per outcome shown when it is
  over), finds may carry a text per language (a letter, a diary page, shown
  when found and kept in the album) and belong to a campaign (found only
  while it runs). A public chronicle (`/chronik.html`, `GET /api/chronicle`)
  tells what happened; whoever missed a chapter's end sees it on the next
  visit. All of it is written in the editor.
- The chronicle tells the past first: finished and running chapters; chapters
  still to come are folded to their dates, and a campaign that hasn't
  started goes last. It is linked where people look: in the chapter bar, at
  the top of "Neuigkeiten" and in the footer, not only under Hilfe.
- News flashes per region (`game.notices`, editor tab "Aktuelles"): a title,
  a text, a link and a period; the app shows a running one once as a card
  and keeps it under "what's new".
- Finds can have a period (migration V4, `starts`/`ends`, either end open):
  they play only within it. A find of a campaign needs both, the campaign
  running and the period fitting, so a letter can wait for its reveal in the
  story and a seasonal find can come and go. Editor: "Spielbar ab / bis" at
  the find, the period in the list.
- Live claims, anchors and sparks survive a restart or an idle unload
  (`game.live_sources`, migration V2): saved anonymously when the region
  stops, put back aged by the pause.
- The first chapter of a campaign (where levels are on) says that everyone
  starts again at level 1: in the chapter bar, the intro card and the
  chapter's card (`resets` on the chapter message). The help page says so
  in its chapter bullet too.
- Editor: the chosen city survives a reload (URL and device); zones, hotspot
  sites and finds are sorted by name as the city reads it, with a search
  field at each list (one search, kept in step). Finds can be filtered by
  campaign (all, the ones that always play, or one campaign). "Brief hier
  hinzulegen" on an existing find opens a new find at the same spot, bound
  to the newest campaign, for a letter. A draft chapter due within a week is
  marked in red: drafts don't play.
- The city list has a search field; from the globe, "Zu mir" takes a player
  straight back to where they stand (watchers still go back to the city).

### Changed

- The album of finds moved from a folded box in the profile to the ranking
  page: one row per collection with a progress bar, the count and the names,
  letters still open on tap. When collecting is off, the row says so with a
  button to switch it on. The collect switch itself stays in the profile under
  privacy; the player's permanent sources with their inscriptions moved from
  the ranking page to the profile.

### Fixed

- The chapter bar's "ab Di 17:59": the relative seconds arrive a moment after
  the server computed them and the time formatter cuts the seconds, so a
  chapter set for 18:00 read 17:59. Start and end times now round to the
  minute.
- The junction card's "weak enough in about …" was rounded up to 30-second
  steps on the server while the card asked again every second, so it sat at
  "1 min" and flickered against the card's own countdown instead of falling.
  The server now answers to the second.
- A region that woke up between UTC midnight and its local midnight took the
  day's side totals from the wrong day: Europe carried yesterday's into the
  new day, the Americas started at zero under tomorrow's date. Scores now load
  by the region's local day from the start.
- A moved permanent source lost its tag line; it takes it along now.
- Editor: a find added or deleted under Orte appears in the targets of the
  Medien tab at once, no reload needed.

## 0.1.0 – first public version

The game as it runs today, published under the GNU AGPL v3 or later.

### Game

- Fire against water on the real street graph of a city: walk to a junction
  and claim it with the phone's GPS; claims spread along the streets by a
  field simulation that follows the terrain (fire runs uphill, water downhill).
- Sources that grow, age and fall; takeovers, reinforcing at the front,
  encirclement of permanent sources, and areas (blocks of the street graph)
  held by one side.
- Hinterland sparks and supply, permanent sources with tag lines, finds
  (named places worth visiting), hotspots, the day's weather, live rain and
  sun from published weather files, and tides at the waterfront.
- Points, levels and abilities, weekly streaks for players and cities,
  badges, and campaigns: chapters with goals such as claims, fronts, finds,
  points or holding a set of junctions at a given time.
- Watching without playing: anyone can follow a city live, cheer for a side,
  see a time-lapse, and run a kiosk view.

### Privacy

- No accounts and no stored positions: players are random identities with a
  recovery code; analytics are k-anonymous aggregates.
- Proof of work for new identities, per-client limits, position plausibility
  checks.

### Platform

- One Rust server (Bevy ECS, axum) for many cities: each region loads on first
  use and unloads when idle; shards per continent with signed events between
  them.
- Web app (TypeScript, Vite, MapLibre GL) as an installable PWA in seven
  languages (de, en, fr, es, pl, it, pt), self-hosted basemap without
  third-party requests.
- Editor for cities: chapters, inscriptions, places, finds and images, with a
  second person approving new images; a dashboard with aggregate numbers.
  Access by token or by login through a forward-auth proxy (Authelia,
  authentik, …), with roles per city from the login's groups.
- PostGIS schema as one Flyway migration (`V1__init.sql`); import of a region from OpenStreetMap
  (imposm) and Copernicus GLO-30 terrain; rule sets as TOML (`rules/`).
- Container image (`deploy/Dockerfile`).
