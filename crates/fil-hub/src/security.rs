//! HTTP-origin policy and bounded admission budgets. Terminal frames never pass
//! through these budgets: only HTTP requests and WebSocket upgrades do.
use crate::state::AppState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Default)]
pub struct AdmissionBudget(Arc<Mutex<HashMap<(IpAddr, bool), Bucket>>>);

struct Bucket {
    remaining: f64,
    updated: Instant,
}

impl AdmissionBudget {
    pub fn allow(&self, ip: IpAddr, auth: bool, now: Instant) -> bool {
        let mut buckets = self.0.lock().unwrap();
        let key = (ip, auth);
        if !buckets.contains_key(&key) && buckets.len() >= 8192 {
            buckets.retain(|_, b| now.duration_since(b.updated).as_secs() < 60);
            if buckets.len() >= 8192 {
                return false;
            }
        }
        // Generous control-plane burst for reconnecting many terminals; login
        // has a separate, smaller budget and cannot consume the control budget.
        let (capacity, per_second) = if auth { (30.0, 0.5) } else { (600.0, 20.0) };
        let bucket = buckets.entry(key).or_insert(Bucket {
            remaining: capacity,
            updated: now,
        });
        bucket.remaining = (bucket.remaining
            + now.duration_since(bucket.updated).as_secs_f64() * per_second)
            .min(capacity);
        bucket.updated = now;
        if bucket.remaining < 1.0 {
            false
        } else {
            bucket.remaining -= 1.0;
            true
        }
    }
}

fn forwarded_identity(headers: &HeaderMap, peer: IpAddr, trusted: &[IpAddr]) -> (IpAddr, bool) {
    if !trusted.contains(&peer) {
        return (peer, false);
    }
    let secure = headers
        .get("x-forwarded-proto")
        .is_some_and(|v| v == "https");
    // Set by our Cloudflare tunnel. Never use a caller-controlled X-Forwarded-For chain.
    let ip = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(peer);
    (ip, secure)
}

pub async fn protect(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|v| v.0.ip().to_canonical())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let (ip, secure) = forwarded_identity(request.headers(), peer, &state.config.trusted_proxy_ips);
    let require_https =
        reqwest::Url::parse(&state.config.public_url).is_ok_and(|url| url.scheme() == "https");
    let local_health = request.uri().path() == "/health"
        && peer.is_loopback()
        && !request.headers().contains_key("x-forwarded-proto");
    let mut response = if !require_https && !peer.is_loopback() {
        (
            StatusCode::UPGRADE_REQUIRED,
            "Development HTTP is loopback-only",
        )
            .into_response()
    } else if require_https && !secure && !local_health {
        // Never reflect OAuth codes or credentials into Location, nor redirect
        // an already-authenticated plaintext request with its bearer attached.
        if matches!(*request.method(), Method::GET | Method::HEAD)
            && !request.headers().contains_key(header::AUTHORIZATION)
            && request.uri().query().is_none()
        {
            let target = format!(
                "{}{}",
                state.config.public_url.trim_end_matches('/'),
                request.uri().path()
            );
            (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, target)]).into_response()
        } else {
            (StatusCode::UPGRADE_REQUIRED, "HTTPS is required").into_response()
        }
    } else if request.uri().path() != "/health"
        && !state.admission_budget.allow(
            ip,
            request.uri().path().starts_with("/auth/"),
            Instant::now(),
        )
    {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "2")],
            "Please retry shortly",
        )
            .into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if secure {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tower::ServiceExt;

    #[tokio::test]
    async fn https_origin_rejects_spoofing_without_reflecting_credentials_and_preserves_local_health()
     {
        let mut state = crate::state::test_state().await;
        state.config.public_url = "HTTPS://hub.example".into();
        state.config.trusted_proxy_ips = vec!["192.0.2.1".parse().unwrap()];
        let app = axum::Router::new()
            .route("/devices", axum::routing::get(|| async { "ok" }))
            .route("/health", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(state, protect));
        for (ip, path, proto, expected) in [
            ("192.0.2.1", "/devices", "https", StatusCode::OK),
            (
                "192.0.2.1",
                "/devices",
                "http",
                StatusCode::PERMANENT_REDIRECT,
            ),
            (
                "192.0.2.9",
                "/devices",
                "https",
                StatusCode::PERMANENT_REDIRECT,
            ),
            (
                "192.0.2.1",
                "/devices?token=synthetic-secret",
                "http",
                StatusCode::UPGRADE_REQUIRED,
            ),
        ] {
            let request = axum::http::Request::builder()
                .uri(path)
                .header("x-forwarded-proto", proto)
                .extension(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 1234)))
                .body(axum::body::Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(!format!("{:?}", response.headers()).contains("synthetic-secret"));
        }
        let health = axum::http::Request::builder()
            .uri("/health")
            .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(app.oneshot(health).await.unwrap().status(), StatusCode::OK);
    }
    #[tokio::test]
    async fn development_http_is_unavailable_to_remote_peers_even_with_spoofed_headers() {
        let state = crate::state::test_state().await;
        let app = axum::Router::new()
            .route("/devices", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(state, protect));
        for (ip, expected) in [
            ("127.0.0.1", StatusCode::OK),
            ("192.0.2.1", StatusCode::UPGRADE_REQUIRED),
        ] {
            let request = axum::http::Request::builder()
                .uri("/devices")
                .header("x-forwarded-proto", "https")
                .extension(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 1234)))
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                expected
            );
        }
    }
    #[test]
    fn login_abuse_is_bounded_without_blocking_control_or_other_clients() {
        let budget = AdmissionBudget::default();
        let ip = "192.0.2.1".parse().unwrap();
        let now = Instant::now();
        for _ in 0..30 {
            assert!(budget.allow(ip, true, now));
        }
        assert!(!budget.allow(ip, true, now));
        assert!(budget.allow(ip, false, now));
        assert!(budget.allow("192.0.2.2".parse().unwrap(), true, now));
        assert!(budget.allow(ip, true, now + Duration::from_secs(2)));
    }
    #[test]
    fn only_explicit_trusted_proxies_can_assert_https_or_client_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert("cf-connecting-ip", HeaderValue::from_static("192.0.2.9"));
        let peer = "192.0.2.1".parse().unwrap();
        assert_eq!(forwarded_identity(&headers, peer, &[]), (peer, false));
        assert_eq!(
            forwarded_identity(&headers, peer, &[peer]),
            ("192.0.2.9".parse().unwrap(), true)
        );
    }
}
