//! The embedded frontend: static files, SPA fallback and asset-URL rewriting
//! (port of `uiRoot`/`indexHTML` in `server.go`).

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use regex::Regex;
use rust_embed::RustEmbed;

use crate::server::{insert_header, is_read_method, plain_text};

#[derive(RustEmbed)]
#[folder = "../../../web/dist"]
struct Assets;

pub fn response(path: &str, method: &Method, base_path: &str) -> Response {
    if !is_read_method(method) {
        let mut response = plain_text(
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain; charset=utf-8",
            "Method Not Allowed\n",
        );
        insert_header(response.headers_mut(), header::ALLOW, "GET, HEAD");
        return response;
    }

    let relative = path.trim_start_matches('/');
    let (body, content_type) = match Assets::get(relative).filter(|_| !relative.is_empty()) {
        Some(file) => (
            file.data.into_owned(),
            mime_guess::from_path(relative)
                .first_or_octet_stream()
                .to_string(),
        ),
        None => index_html(base_path),
    };

    let length = body.len();
    let mut response = if method == Method::HEAD {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(body))
    };
    *response.status_mut() = StatusCode::OK;
    insert_header(response.headers_mut(), header::CONTENT_TYPE, &content_type);
    insert_header(
        response.headers_mut(),
        header::CONTENT_LENGTH,
        &length.to_string(),
    );
    response
}

fn index_html(base_path: &str) -> (Vec<u8>, String) {
    match Assets::get("index.html") {
        Some(file) => {
            let html = String::from_utf8_lossy(&file.data);
            (
                rewrite_asset_refs(&html, base_path).into_bytes(),
                "text/html; charset=utf-8".to_string(),
            )
        }
        None => (
            PLACEHOLDER.as_bytes().to_vec(),
            "text/html; charset=utf-8".to_string(),
        ),
    }
}

/// Prefix the root-absolute `href`/`src` URLs of a built `index.html` with the
/// mount prefix. Vite already does this when compiled with
/// `OTTERSCOPE_BASE_PATH`; rewriting here keeps a root-built binary usable
/// under a prefix, where a stray `/assets/...` would fetch from the ingress
/// root. Rewriting is idempotent.
pub fn rewrite_asset_refs(html: &str, base_path: &str) -> String {
    if base_path.is_empty() {
        return html.to_string();
    }
    let Ok(pattern) = Regex::new(r#"(href|src)="(/[^"]*)""#) else {
        return html.to_string();
    };
    let prefixed = format!("{base_path}/");
    pattern
        .replace_all(html, |captures: &regex::Captures<'_>| {
            let attribute = &captures[1];
            let reference = &captures[2];
            if reference == base_path || reference.starts_with(&prefixed) {
                captures[0].to_string()
            } else {
                format!(r#"{attribute}="{base_path}{reference}""#)
            }
        })
        .into_owned()
}

/// Shown when the binary was built without the frontend. It carries no style
/// of its own: the UI responses use a strict CSP with no `unsafe-inline`.
const PLACEHOLDER: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>Otterscope</title>
<main>
  <h1>&#129450; Otterscope</h1>
  <p>The server is running &mdash; this binary was just built without the embedded web UI.</p>
  <p>The OTLP receiver and the API work normally: point your agent at the OTLP
     endpoint and query <code>/api/runs</code> or <code>/mcp</code>.</p>
  <p>To get the UI, install a <a href="https://github.com/otterscope/otterscope/releases">release binary</a>
     or the Docker image, or build the frontend first:
     <code>cd web &amp;&amp; npm run build</code> then rebuild.</p>
</main>"#;
