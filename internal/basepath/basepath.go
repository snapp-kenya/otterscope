// Package basepath mounts a whole HTTP application under a path prefix, so one
// instance can live at http://host/otterscope behind a layer-7 router that
// routes by path rather than by hostname.
//
// The prefix is a deployment detail, so it is stripped once at the edge of the
// process: every handler behind Strip still sees root-absolute paths ("/api/runs",
// "/v1/traces") and does not need to know a prefix exists. Requests that fall
// outside the prefix are 404s — never another application's SPA.
package basepath

import (
	"fmt"
	"net/http"
	"path"
	"strings"
)

// Normalize turns a user-supplied base path into the canonical form: "" for the
// root, otherwise "/segment[/segment...]" with no trailing slash. Absent leading
// and trailing slashes are forgiven ("otterscope/" and "/otterscope" both become
// "/otterscope"); dot segments, empty segments and query/fragment characters are
// rejected, because each of them silently changes what a prefix match means.
func Normalize(raw string) (string, error) {
	p := strings.TrimSpace(raw)
	if p == "" || p == "/" {
		return "", nil
	}
	if !strings.HasPrefix(p, "/") {
		p = "/" + p
	}
	p = strings.TrimRight(p, "/")
	if p == "" {
		return "", nil
	}
	if strings.ContainsAny(p, "?# \t\"'<>\\") {
		return "", fmt.Errorf("base path %q contains invalid characters", raw)
	}
	for _, seg := range strings.Split(strings.TrimPrefix(p, "/"), "/") {
		if seg == "" || seg == "." || seg == ".." {
			return "", fmt.Errorf("base path %q has an empty or dot segment", raw)
		}
	}
	return p, nil
}

// Join returns base + p, for building root-absolute links that must carry the
// prefix (share links, redirects). p must start with "/".
func Join(base, p string) string {
	if base == "" {
		return p
	}
	return base + p
}

// Strip returns h mounted under base. base must already be normalized (see
// Normalize); "" returns h unchanged, which is how the default deployment keeps
// its previous routing exactly.
//
// Paths outside base get 404 from the returned handler. Paths inside it are
// rewritten to their root-relative form before h sees them, and a path that
// would need cleaning ("/base/a/../b", "/base//a") is redirected *within* the
// prefix rather than left to ServeMux, whose own clean-path redirect would
// answer with a root-absolute Location and leak the request out of the mount.
func Strip(base string, h http.Handler) http.Handler {
	if base == "" {
		return h
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rest, ok := trim(base, r.URL.Path)
		if !ok {
			http.NotFound(w, r)
			return
		}
		if unclean(rest) {
			target := Join(base, cleanPreservingSlash(rest))
			if r.URL.RawQuery != "" {
				target += "?" + r.URL.RawQuery
			}
			http.Redirect(w, r, target, http.StatusMovedPermanently)
			return
		}
		r2 := r.Clone(r.Context())
		r2.URL.Path = rest
		r2.URL.RawPath = "" // EscapedPath recomputes it from the trimmed Path
		h.ServeHTTP(w, r2)
	})
}

// trim maps a request path to its root-relative form, reporting whether it is
// inside base at all. "/base" itself becomes "/" so the mount point serves the
// index directly instead of a redirect.
func trim(base, p string) (string, bool) {
	if p == base {
		return "/", true
	}
	rest, ok := strings.CutPrefix(p, base)
	if !ok || !strings.HasPrefix(rest, "/") {
		return "", false
	}
	return rest, true
}

// unclean reports whether ServeMux would treat p as non-canonical: a dot
// segment, or a doubled slash.
func unclean(p string) bool {
	if strings.Contains(p, "//") {
		return true
	}
	for _, seg := range strings.Split(strings.TrimSuffix(p, "/"), "/") {
		if seg == "." || seg == ".." {
			return true
		}
	}
	return false
}

func cleanPreservingSlash(p string) string {
	clean := path.Clean(p)
	if strings.HasSuffix(p, "/") && !strings.HasSuffix(clean, "/") {
		clean += "/"
	}
	return clean
}
