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

/// The base path Vite baked into the embedded bundle, recorded in `index.html`
/// as a `<meta name="otterscope-base-path">` tag. `None` means the UI was not
/// built (or predates the marker), so there is no bundle to disagree with.
pub fn built_base_path(html: &str) -> Option<String> {
    let marker = r#"name="otterscope-base-path""#;
    let start = html.find(marker)?;
    let tag = &html[start + marker.len()..];
    let tag = &tag[..tag.find('>')?];
    let content = tag.find(r#"content=""#)? + r#"content=""#.len();
    let value = &tag[content..];
    Some(value[..value.find('"')?].to_string())
}

fn base_path_warning_for(configured: &str, html: &str) -> Option<String> {
    let built = built_base_path(html)?;
    if built == configured {
        return None;
    }
    let built_label = if built.is_empty() { "/" } else { &built };
    let configured_label = if configured.is_empty() {
        "/"
    } else {
        configured
    };
    let build_arg = if configured.is_empty() {
        "OTTERSCOPE_BASE_PATH=\"\"".to_string()
    } else {
        format!("OTTERSCOPE_BASE_PATH={configured}")
    };
    Some(format!(
        "the embedded web UI was built for base path {built_label:?} but the server is configured for \
         {configured_label:?}: the UI will not work (its router and API calls use the build-time path). \
         The API, MCP and OTLP receiver still work. Rebuild the image with \
         `--build-arg {build_arg}` so the UI and server agree."
    ))
}

/// Warn when the embedded UI was built for a different base path than the one
/// the server is configured with. The stock image is root-only, so running it
/// with `-base-path` otherwise fails silently in the browser: the HTML loads
/// but its JavaScript calls `/api` and routes at the root.
pub fn base_path_warning(configured: &str) -> Option<String> {
    let index = Assets::get("index.html")?;
    let html = String::from_utf8_lossy(&index.data);
    base_path_warning_for(configured, &html)
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

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIXED: &str = r#"<!doctype html><html><head>
        <meta name="otterscope-base-path" content="/otterscope">
        <script type="module" src="/otterscope/assets/index.js"></script>
    </head></html>"#;

    #[test]
    fn reads_the_build_time_base_path_marker() {
        assert_eq!(built_base_path(PREFIXED).as_deref(), Some("/otterscope"));
        let root =
            r#"<meta name="otterscope-base-path" content=""><script src="/assets/x.js"></script>"#;
        assert_eq!(built_base_path(root).as_deref(), Some(""));
        assert_eq!(
            built_base_path(r#"<script src="/assets/x.js"></script>"#),
            None,
            "a bundle without the marker must not be guessed at"
        );
    }

    #[test]
    fn warns_when_configured_and_built_base_paths_disagree() {
        assert!(base_path_warning_for("/otterscope", PREFIXED).is_none());

        let warning = base_path_warning_for("", PREFIXED).expect("mismatch warns");
        assert!(
            warning.contains("built for base path \"/otterscope\""),
            "{warning}"
        );
        assert!(warning.contains("configured for \"/\""), "{warning}");
        assert!(
            warning.contains("--build-arg OTTERSCOPE_BASE_PATH=\"\""),
            "{warning}"
        );
        assert!(warning.contains("still work"), "{warning}");

        let root = r#"<meta name="otterscope-base-path" content="">"#;
        let warning = base_path_warning_for("/otterscope", root).expect("mismatch warns");
        assert!(
            warning.contains("--build-arg OTTERSCOPE_BASE_PATH=/otterscope"),
            "{warning}"
        );
    }
}
