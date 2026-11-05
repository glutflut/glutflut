# tools

Helpers for the repository, not part of the server or the app.

| Path | What | Needs |
| --- | --- | --- |
| `notice.sh` | regenerates `NOTICE` (licences of the shipped Rust crates and npm packages) | cargo-about, `web/node_modules` |
| `bots.mjs` | simulated players for a test server (`ELEMENTS_ALLOW_SIM=1`), counted as synthetic data | node 22 |
