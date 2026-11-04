//! Browser-facing gate and response hardening (port of `guard.go`).
//!
//! Two independent checks protect the API from a page the user visits:
//!
//! - the Origin allow-list stops a foreign page riding the user's network
//!   position to this API;
//! - the Host allow-list stops DNS rebinding, where the attacker's page is both
//!   the Origin and the Host and the browser omits Origin on a same-origin GET.
//!
//! Non-browser clients send no Origin, so only the Host rule applies to them.

use std::collections::HashSet;

use axum::http::{HeaderMap, Method, Request, header};
use axum::response::Response;

use super::json_error;

pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'";

pub fn stamp_security_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        header::HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
}

/// The startup snapshot of `OTTERSCOPE_ALLOWED_ORIGINS`.
#[derive(Debug, Default)]
pub struct AllowList {
    origins: HashSet<String>,
    hosts: HashSet<String>,
}

impl AllowList {
    pub fn parse(raw: &str) -> Self {
        let mut list = AllowList::default();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let Some((scheme, host, port)) = split_origin(entry) else {
                continue;
            };
            list.origins.insert(origin_key(scheme, host, port));
            list.hosts.insert(host.to_string());
        }
        list
    }

    /// Accept a Host header naming loopback, an IP literal, or a configured
    /// origin's host. The port is ignored: a proxy may rewrite it.
    pub fn host_allowed(&self, hostport: &str) -> bool {
        let host = hostname(hostport);
        if host.is_empty() {
            return false;
        }
        if is_loopback_host(&host) || is_ip(&host) {
            return true;
        }
        self.hosts.contains(&host)
    }

    /// Accept loopback origins, configured origins, and a same-origin request
    /// to an IP-literal host. Only http(s) is accepted: `null` and extension
    /// origins are refused.
    pub fn origin_allowed(&self, origin: &str, request_host: &str) -> bool {
        let Some((scheme, host, port)) = split_origin(origin) else {
            return false;
        };
        if is_loopback_host(host) {
            return true;
        }
        if self.origins.contains(&origin_key(scheme, host, port)) {
            return true;
        }
        // Same page, same address: the UI on http://192.168.1.10:80 calling
        // its own API. An attacker's page is a different origin, and a host
        // reached by IP cannot be DNS-rebound.
        let request_host = hostname(request_host);
        if is_ip(&request_host) && host.eq_ignore_ascii_case(&request_host) {
            return true;
        }
        false
    }
}

/// Check the request against the allow-lists and the JSON-only rule for API
/// writes. `path` is already root-relative.
pub fn check(allow: &AllowList, req: &Request<axum::body::Body>, path: &str) -> Option<Response> {
    if !guarded_path(path) {
        return None;
    }
    let host = header_str(req, header::HOST);
    if !allow.host_allowed(&host) {
        return Some(json_error(
            axum::http::StatusCode::FORBIDDEN,
            "host not allowed",
        ));
    }
    let origin = header_str(req, header::ORIGIN);
    if !origin.is_empty() && !allow.origin_allowed(&origin, &host) {
        return Some(json_error(
            axum::http::StatusCode::FORBIDDEN,
            "origin not allowed",
        ));
    }
    if api_write(req.method(), path)
        && content_length_nonzero(req)
        && !json_media_type(&header_str(req, header::CONTENT_TYPE))
    {
        return Some(json_error(
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "writes must be application/json",
        ));
    }
    None
}

fn header_str(req: &Request<axum::body::Body>, name: header::HeaderName) -> String {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// `/metrics` is not guarded: auth protects it like any other token read.
pub fn guarded_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/mcp"
}

fn api_write(method: &Method, path: &str) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) && path.starts_with("/api/")
}

fn content_length_nonzero(req: &Request<axum::body::Body>) -> bool {
    match req.headers().get(header::CONTENT_LENGTH) {
        Some(value) => value.to_str().map(|v| v.trim() != "0").unwrap_or(true),
        None => req
            .headers()
            .get(header::TRANSFER_ENCODING)
            .is_some_and(|v| v.to_str().map(|s| !s.is_empty()).unwrap_or(true)),
    }
}

/// Tolerate parameters (`; charset=utf-8`) on the media type.
fn json_media_type(content_type: &str) -> bool {
    let media = content_type.split(';').next().unwrap_or("").trim();
    media.eq_ignore_ascii_case("application/json")
}

/// Whether `path` would need cleaning: a dot segment or a doubled slash.
pub fn unclean(path: &str) -> bool {
    if path.contains("//") {
        return true;
    }
    path.trim_end_matches('/')
        .split('/')
        .any(|seg| seg == "." || seg == "..")
}

pub fn clean_preserving_slash(path: &str) -> String {
    let trailing = path.ends_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    let mut clean = format!("/{}", out.join("/"));
    if trailing && !clean.ends_with('/') {
        clean.push('/');
    }
    clean
}

/// Split an absolute http(s) origin into (scheme, host, port). A relative
/// value, a non-http scheme, or a missing host is rejected.
fn split_origin(raw: &str) -> Option<(&str, &str, Option<u16>)> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" => "http",
        "https" => "https",
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = split_authority(authority)?;
    if host.is_empty() {
        return None;
    }
    Some((scheme, host, port))
}

/// Split an authority into host and port, stripping IPv6 brackets.
fn split_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) if !p.is_empty() => Some(p.parse().ok()?),
            Some(_) => None,
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((host, port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse::<u16>() {
            Ok(port) => Some((host, Some(port))),
            Err(_) => None,
        },
        Some(_) => Some((authority, None)),
        None => Some((authority, None)),
    }
}

fn origin_key(scheme: &str, host: &str, port: Option<u16>) -> String {
    let port = match (scheme, port) {
        ("http", Some(80)) | ("https", Some(443)) => None,
        (_, p) => p,
    };
    let host = host.to_ascii_lowercase();
    match port {
        Some(port) if host.contains(':') => format!("{scheme}://[{host}]:{port}"),
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

/// Strip a port and IPv6 brackets from a Host header value.
pub fn hostname(hostport: &str) -> String {
    let value = hostport.trim().to_ascii_lowercase();
    let host = if let Some(rest) = value.strip_prefix('[') {
        rest.split_once(']')
            .map(|(host, _)| host.to_string())
            .unwrap_or(value.clone())
    } else {
        match value.rsplit_once(':') {
            Some((host, port))
                if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) =>
            {
                host.to_string()
            }
            _ => value.clone(),
        }
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_string()
}

fn is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host == "::1" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}
