//! The player's score row: name, hidden, deletion, recovery codes, finds
//! opt-in, the test-server side switch, and what other regions tell us.

use bevy::prelude::*;

use elements_shared::prelude::*;

use crate::gateway::FromWeb;
use crate::score::ScoreSlot;
use crate::web::{send, send_finds, PlayerQuery, ServerMsg, WebClients};

use super::{Ctx, Game};

fn slot_of(clients: &WebClients, players: &PlayerQuery, conn: u64) -> Option<u32> {
    let &(e, _) = clients.0.get(&conn)?;
    players.get(e).ok()?.4.map(|s| s.0)
}

pub(super) fn rename(game: &mut Game, conn: u64) {
    if let Some(slot) = slot_of(&game.clients, &game.players, conn) {
        game.scores.rename(slot);
    }
}

pub(super) fn hide(game: &mut Game, conn: u64, on: bool) {
    if let Some(slot) = slot_of(&game.clients, &game.players, conn) {
        game.scores.set_hidden(slot, on);
    }
}

pub(super) fn forget(game: &mut Game, c: &Ctx, conn: u64) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    if let Some(slot) = slot_of(&game.clients, &game.players, conn) {
        if let Some(id) = game.scores.forget(slot) {
            game.keeps.forget(&id);
            game.peers
                .0
                .broadcast(Some(&c.region.slug), || FromWeb::Evict { id });
            crate::cells::evict(id);
        }
        game.commands.entity(*e).remove::<ScoreSlot>();
        info!(conn, "score deleted on request");
    }
    send(out, &ServerMsg::Forgotten);
}

pub(super) fn code(game: &mut Game, conn: u64) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    if let Some(code) = game
        .players
        .get(*e)
        .ok()
        .and_then(|(.., slot, _)| slot.map(|s| s.0))
        .and_then(|slot| game.scores.new_code(slot))
    {
        send(out, &ServerMsg::Code { code: &code });
    }
}

pub(super) fn side(game: &mut Game, conn: u64, faction: &str) {
    // Test servers only: a tester plays both sides without a new identity.
    if !game.allow_sim.0 {
        return;
    }
    let Some(f) = Faction::parse(faction) else {
        return;
    };
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    game.commands.entity(*e).insert(f);
    if let Some(slot) = slot_of(&game.clients, &game.players, conn) {
        game.scores.set_faction(slot, f);
    }
    info!(conn, ?f, "side switched (test server)");
    send(
        out,
        &ServerMsg::Side {
            faction: f.as_str(),
        },
    );
}

pub(super) fn level(game: &mut Game, conn: u64, level: u8) {
    // Test servers only: try abilities and challenges of a level.
    if !game.allow_sim.0 {
        return;
    }
    if let Some(slot) = slot_of(&game.clients, &game.players, conn) {
        game.scores.set_level(slot, level);
        info!(conn, level, "level set (test server)");
    }
}

pub(super) fn recovered(game: &mut Game, c: &Ctx, conn: u64, token: &str) {
    let Some((e, out)) = game.clients.0.get(&conn) else {
        return;
    };
    let current = game
        .players
        .get(*e)
        .ok()
        .and_then(|(.., slot, _)| slot.map(|s| s.0));
    // This device's own (new) score gives way to the recovered one.
    if let Some(cur) = current.filter(|&c| Some(c) != game.scores.find(token)) {
        if let Some(id) = game.scores.forget(cur) {
            game.peers
                .0
                .broadcast(Some(&c.region.slug), || FromWeb::Evict { id });
            crate::cells::evict(id);
        }
    }
    info!(conn, "score recovered with code");
    send(out, &ServerMsg::Recovered { token });
}

pub(super) fn collect(game: &mut Game, conn: u64, on: bool) {
    let Some((_, out)) = game.clients.0.get(&conn) else {
        return;
    };
    if let Some(slot) = slot_of(&game.clients, &game.players, conn) {
        game.scores.set_collect(slot, on);
        if let Some(f) = game.finds.as_deref() {
            let now = crate::analytics::now_s();
            let running = game.campaign.running(now);
            send_finds(out, f, &game.scores, slot, &running, now);
        }
    }
}

/// Every region: a player's id changed (recovery code).
pub(super) fn rekey(game: &mut Game, old: &[u8; 32], new: [u8; 32]) {
    game.scores.rekey(old, new);
    game.keeps.rekey(old, new);
}

/// Every other region: a player was deleted.
pub(super) fn evict(game: &mut Game, id: &[u8; 32]) {
    game.scores.evict(id);
    game.keeps.forget(id);
}
