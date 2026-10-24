# Changelog

All notable changes to Glut & Flut. Versions follow
[Semantic Versioning](https://semver.org/); the format is loosely
[Keep a Changelog](https://keepachangelog.com/).

## Unreleased

### Fixed

- Editor: a find added or deleted under Orte appears in the targets of the
  Medien tab at once, no reload needed.
- A region that woke up between UTC midnight and its local midnight took the
  day's side totals from the wrong day: Europe carried yesterday's into the
  new day, the Americas started at zero under tomorrow's date. Scores now load
  by the region's local day from the start.

- A moved permanent source lost its tag line; it takes it along now.

### Added

- Editor: the chosen city survives a reload (URL and device); zones, hotspot
  sites and finds are sorted by name as the city reads it, with a search
  field at each list (one search, kept in step).
- Editor, finds: "Brief hier hinzulegen" on an existing find opens a new
  find at the same spot, bound to the newest campaign, for a letter;
  the fountain or gate itself stays as it is.
- The chronicle is linked where people look: in the chapter bar, at the top
  of "Neuigkeiten" and in the footer, not only under Hilfe.
- Live claims, anchors and sparks survive a restart or an idle unload
  (`game.live_sources`, migration V2): saved anonymously when the region
  stops, put back aged by the pause.
- Story-driven campaigns (migration V3): a chapter may carry a story (an
  intro shown once when it starts, an ending per outcome shown when it is
  over), finds may carry a text per language (a letter, a diary page, shown
  when found and kept in the album) and belong to a campaign (found only
  while it runs). A public chronicle (`/chronik.html`, `GET /api/chronicle`)
  tells what happened; whoever missed a chapter's end sees it on the next
  visit. All of it is written in the editor.
- News flashes per region (`game.notices`, editor tab "Aktuelles"): a title,
  a text, a link and a period; the app shows a running one once as a card
  and keeps it under "what's new".
- The editor marks a draft chapter that is due within a week in red: drafts
  don't play.
- The city list has a search field; from the globe, "Zu mir" takes a player
  straight back to where they stand (watchers still go back to the city).

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
