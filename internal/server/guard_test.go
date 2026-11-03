package server

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/otterscope/otterscope/internal/evals"
	"github.com/otterscope/otterscope/internal/ingest"
	"github.com/otterscope/otterscope/internal/pricing"
)

// newReq builds a request the way a browser talking to the local UI would:
// httptest.NewRequest's default Host (example.com) is not in the Host
// allow-list, so API tests go through here rather than tripping the guard.
func newReq(method, target string, body io.Reader) *http.Request {
	r := httptest.NewRequest(method, target, body)
	r.Host = "localhost:8080"
	return r
}

// jsonReq builds a write the way the UI sends one: a JSON body with the
// matching content type (the guard refuses a body that does not declare JSON).
func jsonReq(method, target, body string) *http.Request {
	r := newReq(method, target, strings.NewReader(body))
	r.Header.Set("Content-Type", "application/json")
	return r
}

// doWith runs one request through the UI handler with extra headers set.
func doWith(t *testing.T, srv *Server, method, target, body string, hdr map[string]string) *httptest.ResponseRecorder {
	t.Helper()
	var r *http.Request
	if body == "" {
		r = newReq(method, target, nil)
	} else {
		r = newReq(method, target, strings.NewReader(body))
	}
	for k, v := range hdr {
		r.Header.Set(k, v)
	}
	w := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	return w
}

// viewCount reads the saved-view list over a same-origin GET, so a test can
// tell a refused write from a write that went through anyway.
func viewCount(t *testing.T, srv *Server) int {
	t.Helper()
	w := doWith(t, srv, http.MethodGet, "/api/views", "", nil)
	if w.Code != http.StatusOK {
		t.Fatalf("GET /api/views = %d (%s)", w.Code, w.Body.String())
	}
	var list struct {
		Views []struct {
			ID int64 `json:"id"`
		} `json:"views"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &list); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	return len(list.Views)
}

// A page on another site must not be able to write through the user's browser,
// and refusing it must not be a no-op: nothing is created (#review).
func TestCrossOriginWriteRefused(t *testing.T) {
	srv, _ := testServer(t)

	for _, origin := range []string{"https://evil.example", "http://attacker.test:8080", "null"} {
		w := doWith(t, srv, http.MethodPost, "/api/views", `{"name":"evil","params":{}}`, map[string]string{
			"Origin":       origin,
			"Content-Type": "application/json",
		})
		if w.Code != http.StatusForbidden {
			t.Errorf("POST with Origin %q = %d, want 403 (body %s)", origin, w.Code, strings.TrimSpace(w.Body.String()))
		}
	}
	if n := viewCount(t, srv); n != 0 {
		t.Fatalf("cross-origin writes created %d views, want 0", n)
	}

	// Reads are refused the same way: a foreign page must not read the data.
	if w := doWith(t, srv, http.MethodGet, "/api/runs", "", map[string]string{"Origin": "https://evil.example"}); w.Code != http.StatusForbidden {
		t.Errorf("cross-origin GET = %d, want 403", w.Code)
	}
}

// DNS rebinding: the attacker's page is both the Origin and the Host, so a
// browser sends no Origin on its same-origin GET. Host alone must refuse it.
func TestReboundHostRefused(t *testing.T) {
	srv, _ := testServer(t)
	r := newReq(http.MethodGet, "/api/runs", nil)
	r.Host = "evil.example:8080"
	w := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("GET /api/runs with rebound Host = %d, want 403 (body %s)", w.Code, w.Body.String())
	}
}

// A form-encoded or plain-text body is the one write shape a browser sends
// cross-site without a preflight, so it is refused outright (#review).
func TestNonJSONWriteRefused(t *testing.T) {
	srv, _ := testServer(t)
	for _, ct := range []string{"text/plain", "application/x-www-form-urlencoded", "multipart/form-data; boundary=x"} {
		w := doWith(t, srv, http.MethodPost, "/api/views", `{"name":"plain","params":{}}`, map[string]string{"Content-Type": ct})
		if w.Code != http.StatusUnsupportedMediaType {
			t.Errorf("POST with Content-Type %q = %d, want 415 (body %s)", ct, w.Code, strings.TrimSpace(w.Body.String()))
		}
	}
	// A write with no Content-Type at all is equally not JSON.
	if w := doWith(t, srv, http.MethodPost, "/api/views", `{"name":"bare","params":{}}`, nil); w.Code != http.StatusUnsupportedMediaType {
		t.Errorf("POST with no Content-Type = %d, want 415", w.Code)
	}
	if n := viewCount(t, srv); n != 0 {
		t.Fatalf("non-JSON writes created %d views, want 0", n)
	}
}

// The same-origin path the UI actually uses still works, including a charset
// parameter on the content type.
func TestSameOriginJSONWriteAccepted(t *testing.T) {
	srv, _ := testServer(t)
	w := doWith(t, srv, http.MethodPost, "/api/views", `{"name":"mine","params":{"range":"24h"}}`, map[string]string{
		"Origin":       "http://localhost:8080",
		"Content-Type": "application/json; charset=utf-8",
	})
	if w.Code != http.StatusCreated {
		t.Fatalf("same-origin POST = %d, want 201 (body %s)", w.Code, w.Body.String())
	}
	if n := viewCount(t, srv); n != 1 {
		t.Fatalf("views after same-origin write = %d, want 1", n)
	}
	// The delete path sends no content type at all and must keep working.
	var view struct {
		ID int64 `json:"id"`
	}
	json.Unmarshal(w.Body.Bytes(), &view)
	if w := doWith(t, srv, http.MethodDelete, "/api/views/"+itoa(view.ID), "", map[string]string{"Origin": "http://localhost:8080"}); w.Code != http.StatusNoContent {
		t.Fatalf("same-origin DELETE = %d, want 204", w.Code)
	}
}

// The UI and the JSON API both carry the hardening headers, whatever the
// status code.
func TestSecurityHeadersSet(t *testing.T) {
	srv, st := testServer(t)
	seedRun(t, st, "h1", 1000)

	cases := []struct {
		method, target, body string
		hdr                  map[string]string
	}{
		{http.MethodGet, "/", "", nil},
		{http.MethodGet, "/api/runs", "", nil},
		{http.MethodGet, "/api/nope", "", nil}, // 404 from the UI catch-all
		{http.MethodPost, "/api/views", "not json", map[string]string{"Content-Type": "text/plain"}}, // 415
	}
	for _, c := range cases {
		w := doWith(t, srv, c.method, c.target, c.body, c.hdr)
		if got := w.Header().Get("Content-Security-Policy"); got != "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'" {
			t.Errorf("%s %s: CSP = %q", c.method, c.target, got)
		}
		if got := w.Header().Get("X-Content-Type-Options"); got != "nosniff" {
			t.Errorf("%s %s: X-Content-Type-Options = %q, want nosniff", c.method, c.target, got)
		}
		if got := w.Header().Get("Referrer-Policy"); got != "no-referrer" {
			t.Errorf("%s %s: Referrer-Policy = %q, want no-referrer", c.method, c.target, got)
		}
	}

	// The index really is HTML, not an error page that happens to carry headers.
	w := doWith(t, srv, http.MethodGet, "/", "", nil)
	if w.Code != http.StatusOK || !strings.HasPrefix(w.Header().Get("Content-Type"), "text/html") {
		t.Fatalf("GET / = %d %q", w.Code, w.Header().Get("Content-Type"))
	}
}

// /metrics is as sensitive as /api/stats, so it takes the same read token.
func TestMetricsRequiresToken(t *testing.T) {
	_, st := testServer(t)
	seedRun(t, st, "m1", 1000)
	authed := New(st, pricing.Default(), evals.Endpoint{}, 0, true, ingest.Options{}, "", "test")
	h := authed.authWrap(authed.uiHandler())

	tok, err := st.CreateReadToken(context.Background(), "prometheus")
	if err != nil {
		t.Fatalf("CreateReadToken: %v", err)
	}

	get := func(path, bearer string) *httptest.ResponseRecorder {
		r := newReq(http.MethodGet, path, nil)
		if bearer != "" {
			r.Header.Set("Authorization", "Bearer "+bearer)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w
	}

	if w := get("/metrics", ""); w.Code != http.StatusUnauthorized {
		t.Errorf("GET /metrics without a token = %d, want 401 (body %s)", w.Code, w.Body.String())
	}
	if w := get("/metrics", tok.Token); w.Code != http.StatusOK || !strings.Contains(w.Body.String(), "otterscope_runs_total") {
		t.Errorf("GET /metrics with a token = %d, want 200 with metrics", w.Code)
	}
	// Health stays open so a liveness probe needs no credential.
	if w := get("/healthz", ""); w.Code != http.StatusOK {
		t.Errorf("GET /healthz = %d, want 200", w.Code)
	}
}

// A deployment behind a proxy is served from its own origin; naming it in
// OTTERSCOPE_ALLOWED_ORIGINS is the documented way to keep the UI working.
func TestAllowedOriginFromEnv(t *testing.T) {
	t.Setenv(allowedOriginsEnv, "https://obs.corp.example")
	_, st := testServer(t)
	srv := New(st, pricing.Default(), evals.Endpoint{}, 0, false, ingest.Options{}, "", "test")

	r := newReq(http.MethodGet, "/api/runs", nil)
	r.Host = "obs.corp.example"
	r.Header.Set("Origin", "https://obs.corp.example")
	w := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("configured origin = %d, want 200 (body %s)", w.Code, w.Body.String())
	}

	// A different host on the same connection is still refused.
	r = newReq(http.MethodGet, "/api/runs", nil)
	r.Host = "obs.corp.example"
	r.Header.Set("Origin", "https://elsewhere.example")
	w = httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("foreign origin = %d, want 403", w.Code)
	}
}

// The prefix mounts the guard too: path checks see root-relative paths, so an
// API write under /otterscope is guarded exactly like one at the root.
func TestGuardUnderBasePath(t *testing.T) {
	srv, _ := testServerAt(t, testBase)
	r := newReq(http.MethodPost, testBase+"/api/views", strings.NewReader(`{"name":"x","params":{}}`))
	r.Header.Set("Origin", "https://evil.example")
	r.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("cross-origin write under prefix = %d, want 403 (body %s)", w.Code, w.Body.String())
	}
}

// A deployment exposed on a LAN address keeps working: an IP literal cannot be
// DNS-rebound, and the page it serves is its own origin.
func TestLANAddressAccepted(t *testing.T) {
	srv, _ := testServer(t)

	// Same origin, no Origin header (a plain navigation or a non-browser client).
	r := newReq(http.MethodGet, "/api/runs", nil)
	r.Host = "192.168.1.10:8317"
	w := httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("GET from LAN address = %d, want 200 (body %s)", w.Code, w.Body.String())
	}

	// The UI calling its own API from that address.
	r = newReq(http.MethodGet, "/api/runs", nil)
	r.Host = "192.168.1.10:8317"
	r.Header.Set("Origin", "http://192.168.1.10:8317")
	w = httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusOK {
		t.Fatalf("same-origin GET from LAN address = %d, want 200", w.Code)
	}

	// A page somewhere else posting to it is still refused.
	r = newReq(http.MethodPost, "/api/views", strings.NewReader(`{"name":"x","params":{}}`))
	r.Host = "192.168.1.10:8317"
	r.Header.Set("Origin", "http://203.0.113.5")
	r.Header.Set("Content-Type", "application/json")
	w = httptest.NewRecorder()
	srv.uiHandler().ServeHTTP(w, r)
	if w.Code != http.StatusForbidden {
		t.Fatalf("cross-origin write to LAN address = %d, want 403", w.Code)
	}
}

// The listeners carry the hardening timeouts, and WriteTimeout stays unset so
// a long-lived SSE stream is never cut off mid-connection.
func TestHTTPServerTimeoutSettings(t *testing.T) {
	srv := httpServer(":0", http.NotFoundHandler())
	if srv.ReadHeaderTimeout <= 0 {
		t.Error("ReadHeaderTimeout must be set: a stalled header is a slowloris hold")
	}
	if srv.ReadTimeout <= 0 {
		t.Error("ReadTimeout must be set")
	}
	if srv.IdleTimeout <= 0 {
		t.Error("IdleTimeout must be set")
	}
	if srv.MaxHeaderBytes <= 0 || srv.MaxHeaderBytes > 1<<20 {
		t.Errorf("MaxHeaderBytes = %d, want a bound well under the 1MB default", srv.MaxHeaderBytes)
	}
	if srv.WriteTimeout != 0 {
		t.Errorf("WriteTimeout = %v: it would cut /api/stream off mid-connection", srv.WriteTimeout)
	}
}
