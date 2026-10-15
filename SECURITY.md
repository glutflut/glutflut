# Security

Please report vulnerabilities privately, not as a public issue:

- GitHub: **Security → Report a vulnerability** on
  <https://github.com/glutflut/glutflut> (private vulnerability reporting), or
- e-mail: tr@scientific-wave.com

Say what you found, how to reproduce it, and what you think the impact is.
Please give us a reasonable time to fix it before you publish anything.

## In scope

- The server (`crates/server`, `crates/shared`): the web gateway, socket
  protocol, ops and editor endpoints, limits and anti-cheat, shard events.
- The web app (`web/`): anything that leaks a player's data or position, lets
  one player act as another, or runs code in someone else's browser.
- The database migrations and import scripts in this repository.
- Privacy: the game is built not to store positions or identify players. A way
  to get either out of the server or its aggregates counts as a vulnerability.

## Out of scope

- Spoofing your own GPS position (a known limit of browser games; reports of
  ways around the plausibility checks are still welcome).
- Denial of service by sheer volume, and findings that need a compromised
  device or a malicious browser extension.
- Instances run by others with this code: report to their operators.

## What happens next

This is a small project, so the response is best effort: we aim to confirm
your report within a week and to tell you what we plan to do. We credit
reporters in the changelog unless you prefer not to be named.
