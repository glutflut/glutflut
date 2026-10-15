//! Logins through the reverse proxy instead of tokens, for the dashboard and
//! the editor. A forward-auth server (Authelia, authentik, oauth2-proxy …)
//! checks the person; the proxy then passes their name and groups as headers,
//! and this server maps the groups to roles per region:
//!
//!   ELEMENTS_AUTH_USER_HEADER=Remote-User      (unset: logins off, tokens only)
//!   ELEMENTS_AUTH_GROUPS_HEADER=Remote-Groups  (comma-separated groups)
//!   ELEMENTS_AUTH_GROUP_PREFIX=elements-       (only these groups count)
//!   ELEMENTS_AUTH_LOGOUT_URL=https://auth.example.org/logout
//!
//! Groups: `<prefix>read|editor|admin` for every region,
//! `<prefix><role>-<region slug>` for one, e.g. `elements-editor-region-konstanz`.
//!
//! The headers are trusted as they come, so the proxy must strip them from
//! every request that didn't pass the forward auth (docs/deploy.md, Logins).

use axum::http::{header, HeaderMap};

use crate::gateway::Role;

/// The login settings from the environment; `None`: logins off.
pub struct Config {
    user: String,
    groups: String,
    prefix: String,
    pub logout: Option<String>,
}

pub fn config() -> Option<&'static Config> {
    static CFG: std::sync::OnceLock<Option<Config>> = std::sync::OnceLock::new();
    CFG.get_or_init(|| {
        let env = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Some(Config {
            user: env("ELEMENTS_AUTH_USER_HEADER")?,
            groups: env("ELEMENTS_AUTH_GROUPS_HEADER").unwrap_or_else(|| "Remote-Groups".into()),
            prefix: env("ELEMENTS_AUTH_GROUP_PREFIX").unwrap_or_else(|| "elements-".into()),
            logout: env("ELEMENTS_AUTH_LOGOUT_URL").filter(|u| u.starts_with("https://")),
        })
    })
    .as_ref()
}

/// A logged-in person: name and what their groups grant (`None`: every region).
pub struct Login {
    pub user: String,
    pub grants: Vec<(Option<String>, Role)>,
}

/// The login of this request, if logins are on, the user header is set and
/// the request comes from our own pages.
pub fn from(cfg: &Config, headers: &HeaderMap) -> Option<Login> {
    let user = text(headers, &cfg.user)?;
    if !same_origin(headers) {
        return None;
    }
    let grants = text(headers, &cfg.groups)
        .map(|g| grants(&g, &cfg.prefix))
        .unwrap_or_default();
    Some(Login { user, grants })
}

fn text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

/// The groups with our prefix as grants; others are ignored.
fn grants(groups: &str, prefix: &str) -> Vec<(Option<String>, Role)> {
    groups
        .split(',')
        .filter_map(|g| g.trim().strip_prefix(prefix))
        .filter_map(|g| {
            let (role, slug) = match g.split_once('-') {
                Some((r, s)) => (r, Some(s)),
                None => (g, None),
            };
            let role = match role {
                "admin" => Role::Admin,
                "editor" => Role::Editor,
                "read" => Role::Read,
                _ => return None,
            };
            match slug {
                None => Some((None, role)),
                Some(s)
                    if !s.is_empty()
                        && s.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
                {
                    Some((Some(s.to_string()), role))
                }
                Some(_) => None,
            }
        })
        .collect()
}

/// Not from another site: the login rides on a cookie the browser sends
/// everywhere, unlike a token in a header. Browsers say where a request comes
/// from (`Sec-Fetch-Site`); older ones at least send `Origin` on writes.
fn same_origin(headers: &HeaderMap) -> bool {
    match headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        Some(site) => site == "same-origin" || site == "none",
        None => {
            let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
            let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
            match (origin, host) {
                (None, _) => true,
                (Some(o), Some(h)) => o.split_once("://").is_some_and(|(_, rest)| rest == h),
                (Some(_), None) => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            user: "Remote-User".into(),
            groups: "Remote-Groups".into(),
            prefix: "elements-".into(),
            logout: None,
        }
    }

    fn req(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn groups_become_grants() {
        let g = grants(
            "elements-read, elements-editor-region-konstanz,other,elements-admin-region-x-y,elements-boss,elements-editor-Bad",
            "elements-",
        );
        assert_eq!(
            g,
            vec![
                (None, Role::Read),
                (Some("region-konstanz".into()), Role::Editor),
                (Some("region-x-y".into()), Role::Admin),
            ]
        );
        assert!(grants("glutflut-admin", "elements-").is_empty());
    }

    #[test]
    fn needs_the_user_header() {
        assert!(from(&cfg(), &req(&[("remote-groups", "elements-admin")])).is_none());
        let l = from(
            &cfg(),
            &req(&[("remote-user", "anna"), ("remote-groups", "elements-admin")]),
        )
        .unwrap();
        assert_eq!(l.user, "anna");
        assert_eq!(l.grants, vec![(None, Role::Admin)]);
        // Logged in, but no group of ours: a login without rights.
        let l = from(&cfg(), &req(&[("remote-user", "bob")])).unwrap();
        assert!(l.grants.is_empty());
    }

    #[test]
    fn refuses_other_sites() {
        let ok = [("remote-user", "anna"), ("sec-fetch-site", "same-origin")];
        assert!(from(&cfg(), &req(&ok)).is_some());
        for site in ["cross-site", "same-site"] {
            assert!(from(
                &cfg(),
                &req(&[("remote-user", "anna"), ("sec-fetch-site", site)])
            )
            .is_none());
        }
        // Without Sec-Fetch-Site: Origin must match Host.
        let own = [
            ("remote-user", "anna"),
            ("origin", "https://a.example"),
            ("host", "a.example"),
        ];
        assert!(from(&cfg(), &req(&own)).is_some());
        let other = [
            ("remote-user", "anna"),
            ("origin", "https://evil.example"),
            ("host", "a.example"),
        ];
        assert!(from(&cfg(), &req(&other)).is_none());
    }
}
