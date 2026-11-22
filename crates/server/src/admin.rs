//! The admin page across shards (`web/admin.html`): the page asks the other
//! shards' servers itself, with the same login or token.
//!
//!   ELEMENTS_ADMIN_PEERS=sa=https://ops-sa.example.org,na=https://ops-na.example.org
//!       (on the page's server: the other shards' admin sites; listed in
//!       `GET /api/ops/regions` and allowed in the page's connect-src)
//!   ELEMENTS_ADMIN_ORIGINS=https://ops.example.org
//!       (on the other shards: the page's origin, allowed to read the admin
//!       endpoints with credentials)
//!
//! With a login (forward auth) the page sends only simple requests (GET, POST
//! with a text body), so no preflight has to pass the proxy's login; a token
//! goes in a header and needs the preflight answered here.

use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};

fn list(var: &str) -> Vec<String> {
    std::env::var(var)
        .ok()
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The other shards' admin sites: (shard, base URL).
pub fn peers() -> &'static [(String, String)] {
    static P: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        list("ELEMENTS_ADMIN_PEERS")
            .into_iter()
            .filter_map(|e| {
                let (s, u) = e.split_once('=')?;
                let u = u.trim();
                (u.starts_with("https://") || u.starts_with("http://"))
                    .then(|| (s.trim().to_string(), u.to_string()))
            })
            .collect()
    })
}

fn origins() -> &'static [String] {
    static O: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    O.get_or_init(|| list("ELEMENTS_ADMIN_ORIGINS"))
}

/// A login from the admin page on another shard: its origin is listed in
/// `ELEMENTS_ADMIN_ORIGINS` (the login's same-origin rule, `login.rs`, would
/// refuse it otherwise).
pub fn trusted(headers: &axum::http::HeaderMap) -> bool {
    trusted_in(headers, origins())
}

fn trusted_in(headers: &axum::http::HeaderMap, allowed: &[String]) -> bool {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    // A sibling site only (same-site), never another site.
    get("sec-fetch-site").is_none_or(|s| s == "same-site")
        && get("origin").is_some_and(|o| allowed.iter().any(|a| a == o))
}

/// The endpoints the admin page reads or writes on another shard.
fn admin_path(p: &str) -> bool {
    matches!(
        p,
        "/api/access"
            | "/api/feedback"
            | "/api/wishes"
            | "/api/ops/regions"
            | "/api/ops/reload"
            | "/api/ops/pending"
            | "/api/ops/media/decide"
            | "/api/ops/mottos"
            | "/api/ops/maintenance"
    ) || p.starts_with("/api/ops/feedback/")
}

/// CORS for the admin endpoints, only for `ELEMENTS_ADMIN_ORIGINS`.
pub async fn cors(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .filter(|o| admin_path(req.uri().path()) && origins().iter().any(|a| a == o))
        .and_then(|o| HeaderValue::from_str(o).ok());
    let Some(origin) = origin else {
        return next.run(req).await;
    };
    let mut res = if req.method() == Method::OPTIONS {
        let mut r = StatusCode::NO_CONTENT.into_response();
        let h = r.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        r
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    h.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    h.append(header::VARY, HeaderValue::from_static("Origin"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_only_the_listed_sibling() {
        let allowed = vec!["https://ops.example.org".to_string()];
        let h = |pairs: &[(&'static str, &str)]| {
            let mut m = axum::http::HeaderMap::new();
            for (k, v) in pairs {
                m.insert(*k, v.parse().unwrap());
            }
            m
        };
        let ops = ("origin", "https://ops.example.org");
        assert!(trusted_in(
            &h(&[ops, ("sec-fetch-site", "same-site")]),
            &allowed
        ));
        assert!(
            trusted_in(&h(&[ops]), &allowed),
            "older browsers: Origin only"
        );
        assert!(!trusted_in(
            &h(&[ops, ("sec-fetch-site", "cross-site")]),
            &allowed
        ));
        assert!(!trusted_in(
            &h(&[
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "same-site")
            ]),
            &allowed
        ));
        assert!(!trusted_in(
            &h(&[("sec-fetch-site", "same-site")]),
            &allowed
        ));
        assert!(
            !trusted_in(&h(&[ops]), &[]),
            "nothing listed: nothing trusted"
        );
    }

    #[test]
    fn admin_paths() {
        assert!(admin_path("/api/ops/regions"));
        assert!(admin_path("/api/ops/feedback/12"));
        assert!(admin_path("/api/feedback"));
        assert!(admin_path("/api/ops/pending"));
        assert!(admin_path("/api/ops/media/decide"));
        assert!(admin_path("/api/ops/maintenance"));
        assert!(!admin_path("/api/ops/media"), "uploads stay same-origin");
        assert!(!admin_path("/api/ops/indoor"), "secrets stay same-origin");
        assert!(!admin_path("/api/ops/chapters"));
    }
}
