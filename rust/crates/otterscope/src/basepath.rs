//! Mount the whole HTTP application under a path prefix.
//!
//! The prefix is a deployment detail, so it is stripped once at the edge of the
//! process: every handler behind [`strip`] sees root-absolute paths
//! (`/api/runs`, `/v1/traces`) and never needs to know a prefix exists.
//! Requests outside the prefix are 404s — never another application's SPA.

/// Canonicalise a user-supplied base path: empty for the root, otherwise
/// `/segment[/segment...]` with no trailing slash. Missing leading and trailing
/// slashes are forgiven; dot segments, empty segments and query/fragment
/// characters are rejected because each silently changes what a prefix match
/// means.
pub fn normalize(raw: &str) -> Result<String, String> {
    let mut p = raw.trim().to_string();
    if p.is_empty() || p == "/" {
        return Ok(String::new());
    }
    if p.contains(['?', '#', ' ', '\t', '"', '\'', '<', '>', '\\']) {
        return Err(format!("base path {raw:?} contains invalid characters"));
    }
    if !p.starts_with('/') {
        p = format!("/{p}");
    }
    let p = p.trim_end_matches('/').to_string();
    if p.is_empty() {
        return Ok(String::new());
    }
    for seg in p.trim_start_matches('/').split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(format!("base path {raw:?} has an empty or dot segment"));
        }
    }
    Ok(p)
}

/// Strip the prefix from a request path. Returns `None` when the path is
/// outside the prefix, which the caller turns into a 404.
pub fn strip<'a>(prefix: &str, path: &'a str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(path);
    }
    if path == prefix {
        return Some("/");
    }
    path.strip_prefix(prefix)
        .filter(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_forgiving_spellings() {
        assert_eq!(normalize("").unwrap(), "");
        assert_eq!(normalize("/").unwrap(), "");
        assert_eq!(normalize("otterscope/").unwrap(), "/otterscope");
        assert_eq!(normalize("/a/b/").unwrap(), "/a/b");
    }

    #[test]
    fn rejects_ambiguous_segments() {
        assert!(normalize("/a//b").is_err());
        assert!(normalize("/a/../b").is_err());
        assert!(normalize("/a?x=1").is_err());
    }

    #[test]
    fn strip_requires_a_segment_boundary() {
        assert_eq!(strip("/otterscope", "/otterscope/api/runs"), Some("/api/runs"));
        assert_eq!(strip("/otterscope", "/otterscope"), Some("/"));
        assert_eq!(strip("/otterscope", "/otterscopeX/api"), None);
        assert_eq!(strip("", "/api"), Some("/api"));
    }
}
