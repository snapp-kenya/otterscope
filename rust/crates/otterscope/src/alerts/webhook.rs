//! Webhook destination safety: the SSRF gate, redirect handling and URL
//! redaction (port of `internal/alerts/webhook.go`).

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{CONTENT_TYPE, LOCATION};
use reqwest::{Method, Url};

/// How many redirects a webhook POST may follow. Every hop is re-validated;
/// the cap only stops a redirect loop from holding a cycle open.
pub const MAX_WEBHOOK_REDIRECTS: usize = 3;

/// Bounds the DNS lookup done while validating a URL, so a slow resolver
/// cannot stall alert creation or a watcher cycle.
pub const WEBHOOK_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Stands in for the path and query of a stored webhook URL.
pub const WEBHOOK_PATH_MASK: &str = "/***";

/// A resolver narrowed to what validation needs. Tests stub it so hostname
/// validation is hermetic and a name can be made to resolve to whatever the
/// test needs.
pub type Lookup =
    Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Result<Vec<IpAddr>>> + Send>> + Send + Sync>;

type GateFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// The "may this URL be POSTed to?" check. It is injected so tests whose
/// webhook is a loopback test server can opt out without weakening production.
pub type WebhookGate = Arc<dyn Fn(String) -> GateFuture + Send + Sync>;

/// Production gate: resolves with the system resolver.
pub fn webhook_gate() -> WebhookGate {
    Arc::new(|raw| Box::pin(async move { validate_webhook_url(&raw).await }))
}

/// Reports whether `raw` is a webhook we are willing to POST to: an absolute
/// http(s) URL whose host resolves only to public addresses.
///
/// It runs when an alert is created and before every POST. The second check is
/// what stops rules that predate the guard — or that were written straight
/// into the database — from turning the watcher into a request forwarder, and
/// it re-resolves at use time so a name cannot be pointed at an internal
/// address after the rule was accepted.
pub async fn validate_webhook_url(raw: &str) -> Result<()> {
    validate_webhook_url_with(raw, &system_lookup()).await
}

/// Validation with an injected resolver, so the production path and its test
/// exercise the same code.
pub async fn validate_webhook_url_with(raw: &str, lookup: &Lookup) -> Result<()> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("webhook URL is empty");
    }
    let url = Url::parse(raw).map_err(|err| {
        if has_scheme(raw) {
            anyhow!("webhook URL is not a valid URL: {err}")
        } else {
            anyhow!("webhook URL must be absolute, e.g. https://hooks.example.com/…")
        }
    })?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => bail!("webhook URL must use http or https, not {scheme:?}"),
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("webhook URL must not embed credentials");
    }
    let Some(host) = url.host_str() else {
        bail!("webhook URL must be absolute, e.g. https://hooks.example.com/…");
    };
    let host = strip_ipv6_brackets(host);
    if host.is_empty() {
        bail!("webhook URL has no host");
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        check_webhook_addr(ip).map_err(|err| anyhow!("webhook URL points at {err:#}"))?;
        return Ok(());
    }

    let addrs = lookup(host)
        .await
        .map_err(|err| anyhow!("webhook URL host {host:?} cannot be resolved: {err:#}"))?;
    if addrs.is_empty() {
        bail!("webhook URL host {host:?} has no addresses");
    }
    // Refuse if any answer is internal: a name that resolves to both a public
    // and an internal address is the rebinding case.
    for ip in addrs {
        check_webhook_addr(ip)
            .map_err(|err| anyhow!("webhook URL host {host:?} resolves to {err:#}"))?;
    }
    Ok(())
}

fn has_scheme(raw: &str) -> bool {
    raw.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

fn strip_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

fn system_lookup() -> Lookup {
    Arc::new(|host: &str| {
        let host = host.to_string();
        Box::pin(async move {
            let lookup = tokio::net::lookup_host((host.as_str(), 0));
            let addrs = tokio::time::timeout(WEBHOOK_LOOKUP_TIMEOUT, lookup)
                .await
                .map_err(|_| anyhow!("lookup timed out"))??;
            Ok(addrs.map(|addr| addr.ip()).collect())
        })
    })
}

/// Ranges that are not "private" to [`Ipv4Addr::is_private`] but still must not
/// be reachable from a webhook.
const RESERVED_V4: &[(u8, u8, u8)] = &[
    (0, 0, 8),     // "this network": routes to localhost on Linux
    (100, 64, 10), // carrier-grade NAT; some clouds put metadata here
    (192, 0, 24),  // IETF protocol assignments
    (198, 18, 15), // benchmarking
    (240, 0, 4),   // reserved, includes broadcast
];

/// Refuses addresses that are not publicly routable. The ranges are the ones
/// that either loop back to the server or reach a cloud metadata service;
/// 169.254.169.254 is covered by link-local.
pub fn check_webhook_addr(ip: IpAddr) -> Result<()> {
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => check_v4(v4),
        IpAddr::V6(v6) => check_v6(v6),
    }
}

fn check_v4(ip: Ipv4Addr) -> Result<()> {
    let octets = ip.octets();
    if ip.is_unspecified() {
        return Err(anyhow!("an unspecified address ({ip})"));
    }
    if ip.is_loopback() {
        return Err(anyhow!("a loopback address ({ip})"));
    }
    if ip.is_link_local() {
        return Err(anyhow!("a link-local address ({ip})"));
    }
    if ip.is_private() {
        return Err(anyhow!("a private address ({ip})"));
    }
    if ip.is_multicast() {
        return Err(anyhow!("a multicast address ({ip})"));
    }
    for &(net, second, prefix) in RESERVED_V4 {
        let matches = match prefix {
            8 => octets[0] == net,
            10 => octets[0] == net && (octets[1] & 0b1100_0000) == second,
            15 => octets[0] == net && (octets[1] & 0b1111_1110) == second,
            24 => octets[0] == net && octets[1] == second,
            4 => octets[0] & 0b1111_0000 == net,
            _ => false,
        };
        if matches {
            return Err(anyhow!("a reserved address ({ip})"));
        }
    }
    Ok(())
}

fn check_v6(ip: Ipv6Addr) -> Result<()> {
    let segments = ip.segments();
    if ip.is_unspecified() {
        return Err(anyhow!("an unspecified address ({ip})"));
    }
    if ip.is_loopback() {
        return Err(anyhow!("a loopback address ({ip})"));
    }
    // fe80::/10: link-local unicast; ff02::/16: link-local multicast.
    if segments[0] & 0xffc0 == 0xfe80 || segments[0] & 0xff0f == 0xff02 {
        return Err(anyhow!("a link-local address ({ip})"));
    }
    // fc00::/7: unique-local.
    if segments[0] & 0xfe00 == 0xfc00 {
        return Err(anyhow!("a private address ({ip})"));
    }
    if ip.is_multicast() {
        return Err(anyhow!("a multicast address ({ip})"));
    }
    Ok(())
}

/// The HTTP client the watcher POSTs with. Redirects are followed manually in
/// [`post_webhook`] so each hop is validated with the same resolver as the
/// initial URL.
pub fn new_webhook_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build webhook client")
}

/// POSTs `payload` to `url`, following at most [`MAX_WEBHOOK_REDIRECTS`] hops
/// and re-validating every hop through `gate`.
///
/// Go rewrites a redirected POST to GET, so a public endpoint cannot bounce
/// the POST into loopback or link-local space and read GET-only internal
/// endpoints; the same rewrite is kept here.
pub async fn post_webhook(
    client: &reqwest::Client,
    url: &str,
    payload: &[u8],
    gate: &WebhookGate,
) -> Result<()> {
    let mut current = Url::parse(url).context("webhook URL is not a valid URL")?;
    let mut method = Method::POST;
    let mut followed = 0usize;
    loop {
        let mut request = client.request(method.clone(), current.clone());
        if method == Method::POST {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(payload.to_vec());
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("post webhook {current}"))?;
        if !response.status().is_redirection() {
            return Ok(());
        }
        let Some(location) = response.headers().get(LOCATION) else {
            return Ok(());
        };
        if followed == MAX_WEBHOOK_REDIRECTS {
            bail!("webhook redirect chain longer than {MAX_WEBHOOK_REDIRECTS} hops");
        }
        let location = location
            .to_str()
            .context("webhook redirect location is not text")?;
        let next = current
            .join(location)
            .context("webhook redirect location is not a URL")?;
        gate(next.to_string())
            .await
            .map_err(|err| anyhow!("refusing redirect: {err:#}"))?;
        if matches!(response.status().as_u16(), 301..=303) {
            method = Method::GET;
        }
        current = next;
        followed += 1;
    }
}

/// Renders a stored webhook URL for a client: scheme, host and port, with the
/// path and query masked. Slack, Discord and most other services put the token
/// that authorises the POST in the path, so that part never leaves the server.
/// A value that cannot be parsed is masked whole rather than echoed.
pub fn redact_webhook_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Ok(url) = Url::parse(raw) else {
        return "***".into();
    };
    let Some(authority) = authority(&url) else {
        return "***".into();
    };
    let mut out = format!("{}://{authority}", url.scheme());
    if !url.path().is_empty() && url.path() != "/" {
        out.push_str(WEBHOOK_PATH_MASK);
    }
    out
}

/// The `host[:port]` part of `url`, with any userinfo removed.
fn authority(url: &Url) -> Option<String> {
    let rest = url.as_str().split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test IP")
    }

    fn stub_lookup(ips: &[&str]) -> Lookup {
        let ips: Vec<IpAddr> = ips.iter().map(|s| ip(s)).collect();
        Arc::new(move |_host| {
            let ips = ips.clone();
            Box::pin(async move { Ok(ips) })
        })
    }

    /// Fails validation if a literal address triggers a lookup: a literal must
    /// be judged without one.
    fn fail_lookup() -> Lookup {
        Arc::new(|host: &str| {
            let host = host.to_string();
            Box::pin(async move { Err(anyhow!("unexpected DNS lookup for {host:?}")) })
        })
    }

    #[tokio::test]
    async fn rejects_internal_literals() {
        for url in [
            "http://127.0.0.1/hook",
            "http://127.1.2.3:8080/hook",
            "http://[::1]/hook",
            "http://[::ffff:127.0.0.1]/hook",
            "http://169.254.169.254/latest/meta-data/",
            "http://[fe80::1]/hook",
            "http://10.1.2.3/hook",
            "http://172.16.0.9/hook",
            "http://192.168.1.1/hook",
            "http://[fd00::1]/hook",
            "http://0.0.0.0:8080/hook",
            "http://100.64.0.1/hook",
        ] {
            assert!(
                validate_webhook_url_with(url, &fail_lookup())
                    .await
                    .is_err(),
                "{url:?} accepted, want refusal"
            );
        }
    }

    #[tokio::test]
    async fn requires_absolute_http_url() {
        for url in [
            "",
            "   ",
            "hooks.slack.com/services/T/B/x",
            "/hook",
            "//hooks.slack.com/x",
            "ftp://93.184.216.34/hook",
            "http:///hook",
            "https://user:pw@93.184.216.34/hook",
        ] {
            assert!(
                validate_webhook_url_with(url, &fail_lookup())
                    .await
                    .is_err(),
                "{url:?} accepted, want refusal"
            );
        }
    }

    #[tokio::test]
    async fn accepts_public_endpoints() {
        let cases: [(&str, Option<&[&str]>); 3] = [
            (
                "https://hooks.slack.com/services/T000/B000/xxxx",
                Some(&["93.184.216.34"]),
            ),
            (
                "https://discord.com/api/webhooks/1/abc",
                Some(&["2606:2800:220:1:248:1893:25c8:1946"]),
            ),
            ("https://93.184.216.34/hook?token=x", None),
        ];
        for (url, ips) in cases {
            let lookup = ips.map_or_else(fail_lookup, stub_lookup);
            assert!(
                validate_webhook_url_with(url, &lookup).await.is_ok(),
                "{url:?} rejected"
            );
        }
    }

    #[tokio::test]
    async fn rejects_internal_resolution_including_rebinding() {
        for ips in [
            &["127.0.0.1"][..],
            &["169.254.169.254"][..],
            &["::1"][..],
            &["10.0.0.7", "93.184.216.34"][..],
        ] {
            assert!(
                validate_webhook_url_with("https://hook.example.com/x", &stub_lookup(ips))
                    .await
                    .is_err(),
                "host resolving to {ips:?} accepted, want refusal"
            );
        }
    }

    #[tokio::test]
    async fn unresolved_or_empty_hosts_are_refused() {
        let unresolved: Lookup = Arc::new(|_| Box::pin(async { Err(anyhow!("no such host")) }));
        assert!(
            validate_webhook_url_with("https://hook.example.com/x", &unresolved)
                .await
                .is_err()
        );
        let empty = stub_lookup(&[]);
        assert!(
            validate_webhook_url_with("https://hook.example.com/x", &empty)
                .await
                .is_err()
        );
    }

    #[test]
    fn error_strings_match_go() {
        let err = check_webhook_addr(ip("127.0.0.1")).expect_err("loopback");
        assert_eq!(err.to_string(), "a loopback address (127.0.0.1)");
        let err = check_webhook_addr(ip("::ffff:127.0.0.1")).expect_err("mapped");
        assert_eq!(err.to_string(), "a loopback address (127.0.0.1)");
        let err = check_webhook_addr(ip("0.0.0.0")).expect_err("unspecified");
        assert_eq!(err.to_string(), "an unspecified address (0.0.0.0)");
        let err = check_webhook_addr(ip("10.0.0.5")).expect_err("private");
        assert_eq!(err.to_string(), "a private address (10.0.0.5)");
        let err = check_webhook_addr(ip("240.0.0.1")).expect_err("reserved");
        assert_eq!(err.to_string(), "a reserved address (240.0.0.1)");
        let err = check_webhook_addr(ip("ff02::1")).expect_err("link-local multicast");
        assert_eq!(err.to_string(), "a link-local address (ff02::1)");
        assert!(check_webhook_addr(ip("2001:db8::1")).is_ok());
    }

    #[tokio::test]
    async fn redirects_into_loopback_are_refused() {
        let gate: WebhookGate = Arc::new(|url| {
            Box::pin(async move {
                if url.contains("127.0.0.1") || url.contains("localhost") {
                    bail!("webhook URL points at a loopback address (127.0.0.1)");
                }
                Ok(())
            })
        });
        let err = crate::alerts::post_webhook(
            &new_webhook_client().expect("client"),
            "http://127.0.0.1:1/hook",
            b"{}",
            &gate,
        )
        .await
        .expect_err("connection to loopback should fail");
        // The initial POST is attempted; only redirect hops go through gate.
        assert!(!err.to_string().contains("refusing redirect"));

        let gate: WebhookGate = Arc::new(|url| {
            Box::pin(async move {
                if url.contains("127.0.0.1") || url.contains("localhost") {
                    bail!("webhook URL points at a loopback address (127.0.0.1)");
                }
                Ok(())
            })
        });
        let target = spawn_redirector("http://127.0.0.1:9/latest/meta-data/").await;
        let err = post_webhook(
            &new_webhook_client().expect("client"),
            &target,
            b"{}",
            &gate,
        )
        .await
        .expect_err("redirect into loopback should be refused");
        assert!(
            err.to_string().contains("refusing redirect"),
            "error = {err}"
        );
    }

    async fn spawn_redirector(location: &str) -> String {
        use axum::routing::post;
        let app = axum::Router::new().route(
            "/hook",
            post({
                let location = location.to_string();
                move || {
                    let location = location.clone();
                    async move {
                        let mut response = axum::response::Response::new(axum::body::Body::empty());
                        *response.status_mut() = axum::http::StatusCode::FOUND;
                        response
                            .headers_mut()
                            .insert(LOCATION, location.parse().expect("location header"));
                        response
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}/hook")
    }

    #[tokio::test]
    async fn redirect_chains_are_capped() {
        let target = spawn_redirector_loop().await;
        let gate: WebhookGate = Arc::new(|_| Box::pin(async { Ok(()) }));
        let err = post_webhook(
            &new_webhook_client().expect("client"),
            &target,
            b"{}",
            &gate,
        )
        .await
        .expect_err("self-redirect should hit the cap");
        assert!(err.to_string().contains("redirect chain"), "error = {err}");
    }

    async fn spawn_redirector_loop() -> String {
        use axum::routing::post;
        let app = axum::Router::new().route(
            "/hook",
            post(|| async {
                let mut response = axum::response::Response::new(axum::body::Body::empty());
                *response.status_mut() = axum::http::StatusCode::TEMPORARY_REDIRECT;
                response
                    .headers_mut()
                    .insert(LOCATION, "/hook".parse().expect("location header"));
                response
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}/hook")
    }

    #[test]
    fn redacts_path_and_query() {
        for (raw, want) in [
            (
                "https://hooks.slack.com/services/T000/B000/SUPERSECRET",
                "https://hooks.slack.com/***",
            ),
            (
                "https://discord.com/api/webhooks/1/SUPERSECRET",
                "https://discord.com/***",
            ),
            (
                "http://93.184.216.34:8443/hook?token=SUPERSECRET",
                "http://93.184.216.34:8443/***",
            ),
            ("https://93.184.216.34", "https://93.184.216.34"),
            ("https://93.184.216.34/", "https://93.184.216.34"),
            ("", ""),
            ("not a url", "***"),
        ] {
            let got = redact_webhook_url(raw);
            assert_eq!(got, want, "redact({raw:?})");
            assert!(!got.contains("SUPERSECRET"), "leaked secret for {raw:?}");
        }
    }

    #[test]
    fn redaction_keeps_the_host_and_masks_the_path() {
        let got = redact_webhook_url("https://hooks.example.com/a/b/c/d");
        assert!(got.contains("hooks.example.com"));
        assert!(got.contains(WEBHOOK_PATH_MASK));
    }
}
