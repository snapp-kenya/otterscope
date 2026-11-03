package server

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/otterscope/otterscope/internal/evals"
	"github.com/otterscope/otterscope/internal/ingest"
	"github.com/otterscope/otterscope/internal/pricing"
	"github.com/otterscope/otterscope/internal/store"
	"github.com/otterscope/otterscope/web"
)

// testBase is the mount point these tests use: the deployment shape the base
// path exists for — one app behind path-based routing, next to other things at
// the listener root.
const testBase = "/otterscope"

// testServerAt is testServer with the whole app mounted under base.
func testServerAt(t *testing.T, base string) (*Server, *store.Store) {
	t.Helper()
	st, err := store.Open(context.Background(), filepath.Join(t.TempDir(), "test.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { st.Close() })
	return New(st, pricing.Default(), evals.Endpoint{}, 0, false, ingest.Options{}, base, "test"), st
}

// serveAt runs one request through h and returns the recorder.
func serveAt(t *testing.T, h http.Handler, method, path, body string) *httptest.ResponseRecorder {
	t.Helper()
	var r *http.Request
	if body == "" {
		r = newReq(method, path, nil)
	} else {
		r = newReq(method, path, strings.NewReader(body))
	}
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

// requireDist skips a UI test when the binary has no embedded frontend, rather
// than letting the placeholder page stand in for a real index.
func requireDist(t *testing.T) {
	t.Helper()
	if _, ok := web.Dist(); !ok {
		t.Skip("web/dist is not built: run `npm --prefix web run build` to enable the UI assertions")
	}
}

// TestBasePathAPIRouteUnderPrefix checks the API answers under the mount, and
// that the payload is byte-identical to the same store served at the root —
// the prefix is a routing detail, never something handlers encode into data.
func TestBasePathAPIRouteUnderPrefix(t *testing.T) {
	srv, st := testServerAt(t, testBase)
	seedRun(t, st, "r-old", 1000)
	seedRun(t, st, "r-new", 2000)

	// One store, two mounts: root and prefix must agree on the body.
	root := New(st, pricing.Default(), evals.Endpoint{}, 0, false, ingest.Options{}, "", "test")

	under := serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/api/runs", "")
	if under.Code != http.StatusOK {
		t.Fatalf("GET %s/api/runs = %d, body %s", testBase, under.Code, under.Body.String())
	}
	atRoot := serveAt(t, root.uiHandler(), http.MethodGet, "/api/runs", "")
	if atRoot.Code != http.StatusOK {
		t.Fatalf("GET /api/runs = %d", atRoot.Code)
	}
	if under.Body.String() != atRoot.Body.String() {
		t.Errorf("prefixed body differs from root body:\n prefix: %s\n root:   %s", under.Body.String(), atRoot.Body.String())
	}

	var resp struct {
		Runs []runJSON `json:"runs"`
	}
	if err := json.Unmarshal(under.Body.Bytes(), &resp); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	if len(resp.Runs) != 2 || resp.Runs[0].ID != "r-new" || resp.Runs[1].ID != "r-old" {
		t.Fatalf("runs under prefix: %+v", resp.Runs)
	}
}

// TestBasePathRootIsNotFound is the isolation half: with the app mounted at
// /otterscope, nothing of it may answer at the listener root, or it would
// shadow whatever else lives there. "/" is the interesting one — the SPA's
// catch-all would happily serve index.html for it if the strip were missing.
func TestBasePathRootIsNotFound(t *testing.T) {
	srv, st := testServerAt(t, testBase)
	seedRun(t, st, "r1", 1000)

	h := srv.uiHandler()
	for _, c := range []struct{ method, path string }{
		{http.MethodGet, "/api/runs"},
		{http.MethodGet, "/healthz"},
		{http.MethodGet, "/metrics"},
		{http.MethodPost, "/mcp"},
		{http.MethodGet, testBase + "api"}, // no separator: not this app
		{http.MethodGet, "/"},
		{http.MethodGet, "/somewhere/unknown"},
	} {
		w := serveAt(t, h, c.method, c.path, "")
		if w.Code != http.StatusNotFound {
			t.Errorf("%s %s = %d, want 404 at the mount edge (body %q)", c.method, c.path, w.Code, w.Body.String())
		}
	}

	// And the mounted equivalents do answer, so the 404s above are the prefix
	// talking and not an empty/broken server.
	if w := serveAt(t, h, http.MethodGet, testBase+"/api/runs", ""); w.Code != http.StatusOK {
		t.Fatalf("GET %s/api/runs = %d", testBase, w.Code)
	}
	if w := serveAt(t, h, http.MethodGet, testBase+"/healthz", ""); w.Code != http.StatusOK {
		t.Fatalf("GET %s/healthz = %d", testBase, w.Code)
	}
}

// TestBasePathServesSPAIndex is the requirement that made the index rewrite
// necessary: the HTML page is served from under the prefix, and the asset URLs
// it carries must too. A root-absolute "/assets/index-*.js" would be fetched
// from the ingress root, i.e. from someone else's application.
func TestBasePathServesSPAIndex(t *testing.T) {
	requireDist(t)
	srv, _ := testServerAt(t, testBase)

	w := serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/", "")
	if w.Code != http.StatusOK {
		t.Fatalf("GET %s/ = %d, body %s", testBase, w.Code, w.Body.String())
	}
	if ct := w.Header().Get("Content-Type"); !strings.HasPrefix(ct, "text/html") {
		t.Fatalf("Content-Type = %q, want text/html", ct)
	}
	body := w.Body.String()
	if !strings.Contains(body, `"/otterscope/assets/`) {
		t.Errorf("index has no prefixed asset reference; body:\n%s", body)
	}
	if strings.Contains(body, `"/assets/`) {
		t.Errorf("index still references a root-absolute asset, which would fetch from the ingress root; body:\n%s", body)
	}
}

// TestBasePathSPAFallbackStaysUnderPrefix: client-side routes are unknown to
// the server and get index.html — under the prefix. The same path outside the
// prefix stays a 404, so the fallback cannot swallow another app's traffic.
func TestBasePathSPAFallbackStaysUnderPrefix(t *testing.T) {
	requireDist(t)
	srv, _ := testServerAt(t, testBase)

	w := serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/somewhere/unknown", "")
	if w.Code != http.StatusOK {
		t.Fatalf("GET %s/somewhere/unknown = %d", testBase, w.Code)
	}
	if ct := w.Header().Get("Content-Type"); !strings.HasPrefix(ct, "text/html") {
		t.Fatalf("fallback Content-Type = %q, want text/html", ct)
	}
	if !strings.Contains(w.Body.String(), `id="root"`) {
		t.Errorf("fallback did not return the SPA index; body:\n%s", w.Body.String())
	}

	if w := serveAt(t, srv.uiHandler(), http.MethodGet, "/somewhere/unknown", ""); w.Code != http.StatusNotFound {
		t.Errorf("GET /somewhere/unknown = %d, want 404 (the fallback must not escape the mount)", w.Code)
	}
}

// TestBasePathShareLinkIsPrefixed: a share URL is handed to a human, so the
// prefix has to be baked into it — "/s/<token>" would point at the ingress
// root. The /s/<token> page itself is a client-side route (the SPA then loads
// /api/shared/<token>), so the data assertion goes through the API path.
func TestBasePathShareLinkIsPrefixed(t *testing.T) {
	srv, st := testServerAt(t, testBase)
	seedRun(t, st, "r1", 1000)

	w := serveAt(t, srv.uiHandler(), http.MethodPost, testBase+"/api/runs/r1/share", "")
	if w.Code != http.StatusCreated {
		t.Fatalf("mint share = %d, body %s", w.Code, w.Body.String())
	}
	var mint struct{ Token, URL string }
	if err := json.Unmarshal(w.Body.Bytes(), &mint); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	if !strings.HasPrefix(mint.URL, testBase+"/s/") {
		t.Fatalf("share URL = %q, want it to start with %q", mint.URL, testBase+"/s/")
	}
	if mint.URL != testBase+"/s/"+mint.Token {
		t.Errorf("share URL = %q, token = %q", mint.URL, mint.Token)
	}

	// The route a browser opens resolves under the prefix instead of 404ing.
	if w := serveAt(t, srv.uiHandler(), http.MethodGet, mint.URL, ""); w.Code != http.StatusOK {
		t.Errorf("GET %s = %d, want 200 (the SPA serves the share page)", mint.URL, w.Code)
	}
	// ... and the run behind it is reachable under the prefix too.
	w = serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/api/shared/"+mint.Token, "")
	if w.Code != http.StatusOK {
		t.Fatalf("GET %s/api/shared/{token} = %d, body %s", testBase, w.Code, w.Body.String())
	}
	var shared struct {
		Run runJSON `json:"run"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &shared); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	if shared.Run.ID != "r1" {
		t.Errorf("shared run = %q, want r1", shared.Run.ID)
	}
}

// TestBasePathAuthAppliesToPrefix: -read-auth decides by app path, and behind a
// prefix the request path still carries the mount, so the exemption/matching
// logic has to strip it before comparing. Health stays open; the API does not.
func TestBasePathAuthAppliesToPrefix(t *testing.T) {
	_, st := testServerAt(t, testBase)
	seedRun(t, st, "r1", 1000)
	srv := New(st, pricing.Default(), evals.Endpoint{}, 0, true, ingest.Options{}, testBase, "test")
	h := srv.authWrap(srv.uiHandler())

	tok, err := st.CreateReadToken(context.Background(), "script")
	if err != nil {
		t.Fatalf("CreateReadToken: %v", err)
	}

	get := func(path, bearer string) int {
		r := newReq(http.MethodGet, path, nil)
		if bearer != "" {
			r.Header.Set("Authorization", "Bearer "+bearer)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code
	}

	if c := get(testBase+"/api/runs", ""); c != http.StatusUnauthorized {
		t.Errorf("%s/api/runs without token = %d, want 401", testBase, c)
	}
	if c := get(testBase+"/api/runs", "nope"); c != http.StatusUnauthorized {
		t.Errorf("%s/api/runs with bad token = %d, want 401", testBase, c)
	}
	if c := get(testBase+"/api/runs", tok.Token); c != http.StatusOK {
		t.Errorf("%s/api/runs with token = %d, want 200", testBase, c)
	}
	// The health check must stay reachable by a load balancer that knows
	// nothing about the token.
	if c := get(testBase+"/healthz", ""); c != http.StatusOK {
		t.Errorf("%s/healthz = %d, want 200 (must stay exempt)", testBase, c)
	}
}

// TestBasePathMCPUnderPrefix: MCP clients are pointed at a URL and POST to it,
// so the endpoint moves with the mount like every other route.
func TestBasePathMCPUnderPrefix(t *testing.T) {
	srv, _ := testServerAt(t, testBase)

	body := `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"c","version":"1"}}}`
	w := serveAt(t, srv.uiHandler(), http.MethodPost, testBase+"/mcp", body)
	if w.Code != http.StatusOK {
		t.Fatalf("POST %s/mcp = %d, body %s", testBase, w.Code, w.Body.String())
	}
	var resp struct {
		Result struct {
			ProtocolVersion string `json:"protocolVersion"`
		} `json:"result"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("bad JSON-RPC response: %v", err)
	}
	if resp.Result.ProtocolVersion != "2025-06-18" {
		t.Errorf("negotiated version = %q, want 2025-06-18", resp.Result.ProtocolVersion)
	}
}

// TestBasePathRedirectStaysInsidePrefix runs the redirect through the real
// handler stack. httptest.NewRequest does not clean the path (it goes through
// ReadRequest/url.Parse), so the dot segment survives to the server, which is
// exactly the case where ServeMux's own cleaning would answer with a
// root-absolute Location and leak the client out of the mount.
func TestBasePathRedirectStaysInsidePrefix(t *testing.T) {
	srv, _ := testServerAt(t, testBase)

	w := serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/a/../b", "")
	if w.Code != http.StatusMovedPermanently {
		t.Fatalf("GET %s/a/../b = %d, want 301", testBase, w.Code)
	}
	if got := w.Header().Get("Location"); got != testBase+"/b" {
		t.Errorf("Location = %q, want %q", got, testBase+"/b")
	}

	w = serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"//b", "")
	if w.Code != http.StatusMovedPermanently {
		t.Fatalf("GET %s//b = %d, want 301", testBase, w.Code)
	}
	if got := w.Header().Get("Location"); got != testBase+"/b" {
		t.Errorf("Location = %q, want %q", got, testBase+"/b")
	}
}

// TestBasePathNormalizedInNew: basePath reaches New from places that never went
// through config.Load (tests, embedders, a future subcommand). A trailing slash
// or a missing one used to mount the app at a prefix that matches no request —
// a dead server that still looks healthy. New normalizes instead.
func TestBasePathNormalizedInNew(t *testing.T) {
	for _, spelling := range []string{"/otterscope/", "otterscope", "  /otterscope  "} {
		srv, st := testServerAt(t, spelling)
		seedRun(t, st, "r1", 1000)
		if w := serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/api/runs", ""); w.Code != http.StatusOK {
			t.Errorf("base %q: GET %s/api/runs = %d, want 200 (the mount must be the canonical prefix)", spelling, testBase, w.Code)
		}
		if w := serveAt(t, srv.uiHandler(), http.MethodGet, testBase+"/healthz", ""); w.Code != http.StatusOK {
			t.Errorf("base %q: GET %s/healthz = %d, want 200", spelling, testBase, w.Code)
		}
	}
}

// TestBasePathOutsideMountIs404NotUnauthorized: with -read-auth on, a path
// outside the mount must not answer 401 — that would advertise this app's API
// at the ingress root, where another service may live. Outside is 404, before
// the token is even considered.
func TestBasePathOutsideMountIs404NotUnauthorized(t *testing.T) {
	_, st := testServerAt(t, testBase)
	srv := New(st, pricing.Default(), evals.Endpoint{}, 0, true, ingest.Options{}, testBase, "test")
	h := srv.authWrap(srv.uiHandler())

	tok, err := st.CreateReadToken(context.Background(), "script")
	if err != nil {
		t.Fatalf("CreateReadToken: %v", err)
	}
	for _, bearer := range []string{"", tok.Token} {
		r := newReq(http.MethodGet, "/api/runs", nil)
		if bearer != "" {
			r.Header.Set("Authorization", "Bearer "+bearer)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		if w.Code != http.StatusNotFound {
			t.Errorf("GET /api/runs outside the mount (token %q) = %d, want 404", bearer, w.Code)
		}
	}
}
