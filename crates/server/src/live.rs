//! Live sources across a restart: when a region's app stops (deploy, idle
//! unload) its claims, anchors and sparks go to `game.live_sources`; the next
//! start of the region takes them back (the row is deleted on load) and puts
//! them on the map, aged by the time in between. Permanent sources have their
//! own table (`keep.rs`), natural ones come from the graph.
//!
//! Anonymous on purpose: junction, side, age and strength, never who placed
//! a source (that would be a stored position). A restored claim pays its
//! holding points to nobody for the minutes it still has.

use std::time::Duration;

use bevy::prelude::*;
use crossbeam_channel::Sender;
use elements_shared::prelude::*;
use serde::{Deserialize, Serialize};

use crate::analytics::now_s;
use crate::sim::{ElementSource, NodeById, SourceKind};

/// A state older than this is not restored (a crash, a long pause): its
/// sources would have expired anyway.
const MAX_AGE_S: f64 = 3600.0;

/// One saved source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub node: i64,
    pub fire: bool,
    pub kind: u8,
    pub life: f32,
    pub age: f32,
    pub strength: f32,
    pub ramp: f32,
    pub built: f32,
    pub mult: f32,
    pub newcomer: bool,
    pub supply: f32,
}

fn kind_code(k: SourceKind) -> u8 {
    match k {
        SourceKind::Claim => 0,
        SourceKind::Anchor => 1,
        SourceKind::Spark => 2,
        SourceKind::Keep => 3,
    }
}

fn kind_of(c: u8) -> Option<SourceKind> {
    Some(match c {
        0 => SourceKind::Claim,
        1 => SourceKind::Anchor,
        2 => SourceKind::Spark,
        _ => return None,
    })
}

/// What the save keeps of a source; `None` for natural and permanent ones.
pub fn row(node: i64, s: &ElementSource) -> Option<Row> {
    let life = s.life.filter(|_| s.kind != SourceKind::Keep)?;
    Some(Row {
        node,
        fire: s.faction == Faction::Fire,
        kind: kind_code(s.kind),
        life,
        age: s.age,
        strength: s.strength,
        ramp: s.ramp,
        built: s.built,
        mult: s.mult,
        newcomer: s.newcomer,
        supply: s.supply,
    })
}

/// The source back from a row, aged by `gap_s` seconds; `None` once expired.
pub fn source(r: &Row, gap_s: f32) -> Option<ElementSource> {
    let kind = kind_of(r.kind)?;
    let age = r.age + gap_s.max(0.0);
    if age >= r.life {
        return None;
    }
    Some(ElementSource {
        faction: if r.fire {
            Faction::Fire
        } else {
            Faction::Water
        },
        life: Some(r.life),
        age,
        strength: r.strength,
        by: None,
        ramp: r.ramp,
        built: r.built + gap_s.max(0.0),
        mult: r.mult,
        newcomer: r.newcomer,
        kind,
        supply: r.supply,
        motto: 0,
        sieged: false,
        linked: false,
    })
}

/// The region's saved sources (loaded on the loader thread) and its writer.
#[derive(Resource, Default)]
pub struct Live {
    pending: Option<(f64, Vec<Row>)>,
    tx: Option<crate::writers::LaneTx<Msg>>,
}

pub enum Msg {
    Save(Vec<Row>),
    Barrier(Sender<()>),
}

impl Live {
    /// Take the region's saved state (if any) and start the writer.
    pub fn new(db: Option<String>, region: String) -> Self {
        let Some(db) = db else {
            return Self::default();
        };
        let pending = match take(&db, &region) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "live sources: load failed");
                None
            }
        };
        let key = format!("live {region}");
        let tx = crate::writers::lane(&key, |rx| Writer { db, region, rx });
        Self {
            pending,
            tx: Some(tx),
        }
    }

    /// Write now and wait (the app is stopping).
    fn save_wait(&self, rows: Vec<Row>) -> bool {
        let Some(tx) = &self.tx else { return true };
        let (ack, done) = crossbeam_channel::bounded(1);
        tx.send(Msg::Save(rows)).is_ok()
            && tx.send(Msg::Barrier(ack)).is_ok()
            && done.recv_timeout(Duration::from_secs(10)).is_ok()
    }
}

/// The saved state, deleted in the same go (a crash later can't bring it back
/// twice); `None` without one or when too old.
fn take(db: &str, region: &str) -> Result<Option<(f64, Vec<Row>)>, Box<dyn std::error::Error>> {
    let mut c = crate::db::connect(db)?;
    let row = c.query_opt(
        "DELETE FROM game.live_sources WHERE region = $1 \
         RETURNING extract(epoch FROM saved_at)::float8, sources::text",
        &[&region],
    )?;
    let Some(r) = row else { return Ok(None) };
    let at: f64 = r.get(0);
    if now_s() - at > MAX_AGE_S {
        return Ok(None);
    }
    let rows: Vec<Row> = serde_json::from_str(r.get(1))?;
    info!(
        sources = rows.len(),
        age_s = (now_s() - at) as i64,
        "live sources loaded"
    );
    Ok(Some((at, rows)))
}

struct Writer {
    db: String,
    region: String,
    rx: crossbeam_channel::Receiver<Msg>,
}

impl crate::writers::Lane for Writer {
    fn run(&mut self, pg: &mut crate::writers::Db) -> bool {
        let db = Some(self.db.as_str());
        let mut save = None;
        let mut acks = Vec::new();
        let open = loop {
            match self.rx.try_recv() {
                Ok(Msg::Save(rows)) => save = Some(rows),
                Ok(Msg::Barrier(a)) => acks.push(a),
                Err(crossbeam_channel::TryRecvError::Empty) => break true,
                Err(crossbeam_channel::TryRecvError::Disconnected) => break false,
            }
        };
        if let Some(rows) = save {
            if let Some(c) = pg.get(db) {
                let json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into());
                if let Err(e) = c.execute(
                    "INSERT INTO game.live_sources (region, saved_at, sources) VALUES ($1, now(), $2::text::jsonb) \
                     ON CONFLICT (region) DO UPDATE SET saved_at = now(), sources = EXCLUDED.sources",
                    &[&self.region, &json],
                ) {
                    pg.failed(db, "live sources write", &e);
                }
            }
        }
        for a in acks {
            let _ = a.send(());
        }
        open
    }
}

/// Put the saved sources back on the map (once the nodes exist).
pub fn restore(mut commands: Commands, mut live: ResMut<Live>, node_by_id: Res<NodeById>) {
    let Some((at, rows)) = live.pending.take() else {
        return;
    };
    let gap = (now_s() - at) as f32;
    let (mut put, mut gone) = (0, 0);
    for r in &rows {
        match (node_by_id.get(&r.node), source(r, gap)) {
            (Some(&e), Some(s)) => {
                commands.entity(e).insert(s);
                put += 1;
            }
            _ => gone += 1,
        }
    }
    info!(put, gone, "live sources restored");
}

/// When the app stops: save what is live, once.
pub fn save_on_stop(
    live: Res<Live>,
    stop: Res<crate::Stop>,
    q: Query<(&GeoNode, &ElementSource)>,
    mut done: Local<bool>,
) {
    use std::sync::atomic::Ordering::Relaxed;
    if *done || !(crate::gateway::SHUTDOWN.load(Relaxed) || stop.0.load(Relaxed)) {
        return;
    }
    *done = true;
    let rows: Vec<Row> = q.iter().filter_map(|(g, s)| row(g.id, s)).collect();
    let n = rows.len();
    let ok = live.save_wait(rows);
    info!(sources = n, written = ok, "live sources saved");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> ElementSource {
        let rules = elements_shared::rules::Rules::default();
        let mut s = ElementSource::claim(
            &rules,
            Faction::Water,
            SourceKind::Anchor,
            540.0,
            Some(7),
            1.1,
            true,
            0.0,
        );
        s.age = 100.0;
        s.built = 100.0;
        s.supply = 0.8;
        s
    }

    #[test]
    fn round_trip_without_the_player() {
        let s = claim();
        let r = row(42, &s).unwrap();
        let json = serde_json::to_value(vec![r.clone()]).unwrap();
        let back: Vec<Row> = serde_json::from_value(json).unwrap();
        let t = source(&back[0], 60.0).unwrap();
        assert_eq!(back[0].node, 42);
        assert_eq!(t.faction, Faction::Water);
        assert_eq!(t.kind, SourceKind::Anchor);
        assert_eq!((t.age, t.built), (160.0, 160.0), "aged by the gap");
        assert_eq!((t.mult, t.supply, t.newcomer), (1.1, 0.8, true));
        assert_eq!(t.by, None, "anonymous: no player behind it");
    }

    #[test]
    fn natural_and_permanent_are_not_saved_expired_not_restored() {
        let rules = elements_shared::rules::Rules::default();
        assert!(row(1, &ElementSource::natural(Faction::Fire, &rules)).is_none());
        let mut keep = claim();
        keep.kind = SourceKind::Keep;
        assert!(row(1, &keep).is_none());
        let r = row(1, &claim()).unwrap();
        assert!(source(&r, 439.0).is_some());
        assert!(
            source(&r, 440.0).is_none(),
            "100 s old + 440 s gap = its 540 s life"
        );
    }
}
