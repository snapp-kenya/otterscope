package basepath

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

// TestNormalize pins the canonical form of a base path and the values it
// refuses. The result is compared byte-for-byte against request paths, so two
// spellings of one mount point must normalize identically; and a value that
// cannot be a single URL-path prefix must be an error at startup rather than a
// mount that silently matches nothing (or matches too much).
func TestNormalize(t *testing.T) {
	accept := []struct{ in, want string }{
		{"", ""},
		{"/", ""},
		{"   ", ""}, // whitespace-only is "unset", not a segment
		{"otterscope", "/otterscope"},
		{"/otterscope", "/otterscope"},
		{"/otterscope/", "/otterscope"},
		{"otterscope/", "/otterscope"},
		{"  /otterscope/  ", "/otterscope"},
		{"/a/b", "/a/b"},
		{"/a/b/", "/a/b"},
		{"/otterscope-2", "/otterscope-2"}, // a dash is an ordinary segment char
	}
	for _, c := range accept {
		got, err := Normalize(c.in)
		if err != nil {
			t.Errorf("Normalize(%q) error: %v", c.in, err)
			continue
		}
		if got != c.want {
			t.Errorf("Normalize(%q) = %q, want %q", c.in, got, c.want)
		}
	}

	reject := []string{
		"//x",          // empty segment
		"/a//b",        // empty segment mid-path
		"/a/../b",      // dot-dot would climb out of the mount
		"/a/./b",       // dot segment
		"/a?b",         // query glued onto the mount point
		"/a#b",         // fragment
		"/otter scope", // a space is not a path segment
		"/a\tb",        // nor is a tab
		`/a"b`,         // a quote would break the hrefs we generate
		"/a<b",         // so would angle brackets
		`/a\b`,         // backslash is not a separator here, but never intended
	}
	for _, in := range reject {
		if got, err := Normalize(in); err == nil {
			t.Errorf("Normalize(%q) = %q, want an error", in, got)
		}
	}
}

// TestStripServesUnderPrefix proves the prefix is removed exactly once, at the
// edge: every handler behind Strip was written root-absolute and must see the
// path it has always seen.
func TestStripServesUnderPrefix(t *testing.T) {
	var gotPath, gotQuery string
	h := Strip("/otterscope", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath, gotQuery = r.URL.Path, r.URL.RawQuery
		w.WriteHeader(http.StatusOK)
	}))

	cases := []struct{ req, wantPath, wantQuery string }{
		{"/otterscope", "/", ""}, // the mount point is the app root, not a redirect
		{"/otterscope/", "/", ""},
		{"/otterscope/api/runs", "/api/runs", ""},
		{"/otterscope/api/runs?limit=2", "/api/runs", "limit=2"},
		{"/otterscope/v1/traces", "/v1/traces", ""},
		{"/otterscope/a/b/c", "/a/b/c", ""},
	}
	for _, c := range cases {
		gotPath, gotQuery = "", ""
		w := httptest.NewRecorder()
		h.ServeHTTP(w, httptest.NewRequest(http.MethodGet, c.req, nil))
		if w.Code != http.StatusOK {
			t.Errorf("%s: status %d, want 200", c.req, w.Code)
			continue
		}
		if gotPath != c.wantPath {
			t.Errorf("%s: handler saw path %q, want %q", c.req, gotPath, c.wantPath)
		}
		// The query is not part of the mount and must survive untouched.
		if gotQuery != c.wantQuery {
			t.Errorf("%s: handler saw query %q, want %q", c.req, gotQuery, c.wantQuery)
		}
	}
}

// TestStripRejectsOutsidePrefix is the other half of the contract: a path that
// is not under the mount must not reach the app at all. The "otterscopeextra"
// case is the one that matters — a plain strings.HasPrefix would accept it and
// hand another application's request to this SPA's router.
func TestStripRejectsOutsidePrefix(t *testing.T) {
	h := Strip("/otterscope", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		t.Errorf("handler called for %q, want a 404 at the edge", r.URL.Path)
		w.WriteHeader(http.StatusOK)
	}))

	for _, p := range []string{
		"/",
		"/other",
		"/otterscopeextra",
		"/otterscopeapi", // the mount's characters with no separator
		"/OtterScope/api/runs",
	} {
		w := httptest.NewRecorder()
		h.ServeHTTP(w, httptest.NewRequest(http.MethodGet, p, nil))
		if w.Code != http.StatusNotFound {
			t.Errorf("%s: status %d, want 404", p, w.Code)
		}
	}
}

// TestStripRedirectStaysInsidePrefix covers non-canonical paths that arrive
// *inside* the mount. ServeMux would clean these itself, but with a
// root-absolute Location, which would move the client out of the mount and
// into whatever else answers at the listener root — so Strip redirects first,
// keeping the prefix and the query.
func TestStripRedirectStaysInsidePrefix(t *testing.T) {
	h := Strip("/otterscope", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		t.Errorf("handler called for unclean path %q, want a redirect", r.URL.Path)
		w.WriteHeader(http.StatusOK)
	}))

	cases := []struct{ req, want string }{
		{"/otterscope/a/../b", "/otterscope/b"},
		{"/otterscope/a/../b?limit=2", "/otterscope/b?limit=2"},
		{"/otterscope//a", "/otterscope/a"},
		{"/otterscope/a//", "/otterscope/a/"},
		{"/otterscope/./a", "/otterscope/a"},
	}
	for _, c := range cases {
		w := httptest.NewRecorder()
		h.ServeHTTP(w, httptest.NewRequest(http.MethodGet, c.req, nil))
		if w.Code != http.StatusMovedPermanently {
			t.Errorf("%s: status %d, want 301", c.req, w.Code)
			continue
		}
		if got := w.Header().Get("Location"); got != c.want {
			t.Errorf("%s: Location %q, want %q", c.req, got, c.want)
		}
	}
}

// markerHandler is comparable, so it can prove Strip("") hands back the very
// handler it was given — the default deployment keeps its previous routing,
// including paths outside any prefix, because there is no prefix.
type markerHandler struct{ calls int }

func (m *markerHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) { m.calls++ }

func TestStripEmptyBaseIsPassthrough(t *testing.T) {
	m := &markerHandler{}
	if got := Strip("", m); got != http.Handler(m) {
		t.Fatalf("Strip(\"\") wrapped the handler; the root deployment must be untouched")
	}
	w := httptest.NewRecorder()
	Strip("", m).ServeHTTP(w, httptest.NewRequest(http.MethodGet, "/otterscope/api/runs", nil))
	if m.calls != 1 {
		t.Fatalf("handler calls = %d, want 1", m.calls)
	}
}
