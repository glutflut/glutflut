-- Shard-wide jobs (globe manifest, shard index, world ranking, cleanup) in
-- one cell at a time: whichever holds this lease (crates/server/src/cells.rs).
CREATE TABLE ops.main_lease (
    shard text PRIMARY KEY,
    cell text NOT NULL,
    until timestamp with time zone NOT NULL
);
