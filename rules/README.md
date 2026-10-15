# Rules

The game rules as TOML, read by the server (`crates/shared/src/rules.rs`). `default.toml` is the set the image runs; it lists only what differs from the built-in defaults.

- Default set: `ELEMENTS_RULES=rules/default.toml` (in the image `/srv/rules/default.toml`).
- More sets: a file that starts with `base = "default.toml"` and lists what it changes. Per region: `geo.regions.rules = '<name>'` picks `<name>.toml` next to `ELEMENTS_RULES`. In an image: put them in `rules.local/` before the build.
- Single values on top of every set: `ELEMENTS_RULES_OVERRIDE="claim_ttl_s=420; field.reach_scale=250"` (a typo stops the server at startup).

Every key and its default: `docs/configuration.md`, section 2.
