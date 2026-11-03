// Package server runs the two HTTP listeners: the UI/API server and the
// OTLP/HTTP receiver. The receiver is a separate listener so the standard
// OTLP port (:4318) works untouched next to the UI.
package server

import (
	"context"
	"encoding/json"
	"errors"
	"io/fs"
	"log/slog"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/otterscope/otterscope/internal/alerts"
	"github.com/otterscope/otterscope/internal/basepath"
	"github.com/otterscope/otterscope/internal/evals"
	"github.com/otterscope/otterscope/internal/ingest"
	"github.com/otterscope/otterscope/internal/mcp"
	"github.com/otterscope/otterscope/internal/pricing"
	"github.com/otterscope/otterscope/internal/store"
	"github.com/otterscope/otterscope/web"
)

// Server hosts the UI/API and the OTLP receiver.
type Server struct {
	st            *store.Store
	prices        *pricing.Table
	judge         evals.Endpoint
	eval          *ingest.Evaluator
	alertInterval time.Duration
	hub           *hub
	readAuth      bool
	ingestOpts    ingest.Options
	basePath      string // "" = listener root
	version       string
	// Browser-facing allow-list, snapshot in New: loopback is always
	// accepted, OTTERSCOPE_ALLOWED_ORIGINS covers a proxied deployment.
	allowedOrigins map[string]struct{}
	allowedHosts   map[string]struct{}
}

// New creates a Server backed by st, pricing LLM calls via prices, judging
// with the server-configured endpoint, and evaluating alerts every
// alertInterval (0 disables the watcher). basePath mounts every route under a
// prefix ("" serves at the listener root); it is normalized here so a caller
// that skipped config.Load cannot end up with a prefix that matches nothing.
func New(st *store.Store, prices *pricing.Table, judge evals.Endpoint, alertInterval time.Duration, readAuth bool, ingestOpts ingest.Options, basePath, version string) *Server {
	if p, err := basepath.Normalize(basePath); err == nil {
		basePath = p
	}
	origins, hosts := envAllowedOrigins()
	return &Server{
		st: st, prices: prices, judge: judge, alertInterval: alertInterval,
		hub: newHub(), readAuth: readAuth, ingestOpts: ingestOpts,
		basePath: basePath, version: version,
		allowedOrigins: origins, allowedHosts: hosts,
	}
}

// Server timeouts. ReadTimeout bounds reading a request (headers plus a small
// JSON body), not writing the response, so a long-lived SSE stream is not
// affected by it; WriteTimeout is deliberately left unset, because that is the
// deadline that would cut /api/stream off mid-connection.
const (
	readHeaderTimeout = 10 * time.Second
	readTimeout       = 30 * time.Second
	idleTimeout       = 120 * time.Second
	maxHeaderBytes    = 64 << 10
)

// httpServer builds a listener with the hardening every listener needs.
func httpServer(addr string, h http.Handler) *http.Server {
	return &http.Server{
		Addr:              addr,
		Handler:           h,
		ReadHeaderTimeout: readHeaderTimeout,
		ReadTimeout:       readTimeout,
		IdleTimeout:       idleTimeout,
		MaxHeaderBytes:    maxHeaderBytes,
	}
}

// Run serves until ctx is canceled, then shuts both listeners down and
// drains the evaluator (before the caller closes the store).
func (s *Server) Run(ctx context.Context, uiAddr, otlpAddr string) error {
	s.eval = ingest.NewEvaluator(s.st, s.judge)
	s.eval.Start()
	defer s.eval.Stop() // drains queued evaluation before Run returns

	if s.alertInterval > 0 {
		watcher := alerts.NewWatcher(s.st, s.alertInterval)
		watcher.Start()
		defer watcher.Stop()
	}

	ui := httpServer(uiAddr, s.authWrap(s.uiHandler()))
	otlp := httpServer(otlpAddr, s.otlpHandler())

	errc := make(chan error, 2)
	go func() { errc <- ui.ListenAndServe() }()
	go func() { errc <- otlp.ListenAndServe() }()
	slog.Info("listening", "ui", uiAddr, "otlp", otlpAddr)

	select {
	case <-ctx.Done():
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		ui.Shutdown(shutdownCtx)
		otlp.Shutdown(shutdownCtx)
		return nil
	case err := <-errc:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	}
}

func (s *Server) uiHandler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /metrics", s.handleMetrics)
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, map[string]string{"status": "ok", "version": s.version})
	})
	mux.HandleFunc("GET /api/runs", s.handleListRuns)
	mux.HandleFunc("GET /api/runs.csv", s.handleRunsCSV)
	mux.HandleFunc("GET /api/runs/{id}", s.handleGetRun)
	mux.HandleFunc("POST /api/runs/{id}/share", s.handleCreateShare)
	mux.HandleFunc("GET /api/runs/{id}/shares", s.handleListShares)
	mux.HandleFunc("DELETE /api/shares/{token}", s.handleDeleteShare)
	mux.HandleFunc("GET /api/shared/{token}", s.handleSharedRun)
	mux.HandleFunc("GET /api/stats", s.handleStats)
	mux.HandleFunc("GET /api/assertions", s.handleListAssertions)
	mux.HandleFunc("POST /api/assertions", s.adminOnly(s.handleCreateAssertion))
	mux.HandleFunc("DELETE /api/assertions/{id}", s.adminOnly(s.handleDeleteAssertion))
	mux.HandleFunc("POST /api/assertions/evaluate", s.adminOnly(s.handleEvaluate))
	mux.HandleFunc("GET /api/stream", s.handleStream)
	mux.HandleFunc("GET /api/audit", s.handleAudit)
	mux.HandleFunc("GET /api/tokens", s.adminOnly(s.handleListTokens))
	mux.HandleFunc("POST /api/tokens", s.adminOnly(s.handleCreateToken))
	mux.HandleFunc("DELETE /api/tokens/{id}", s.adminOnly(s.handleDeleteToken))
	mux.HandleFunc("GET /api/views", s.handleListViews)
	mux.HandleFunc("POST /api/views", s.adminOnly(s.handleCreateView))
	mux.HandleFunc("DELETE /api/views/{id}", s.adminOnly(s.handleDeleteView))
	mux.HandleFunc("GET /api/alerts", s.handleListAlerts)
	mux.HandleFunc("POST /api/alerts", s.adminOnly(s.handleCreateAlert))
	mux.HandleFunc("DELETE /api/alerts/{id}", s.adminOnly(s.handleDeleteAlert))
	mux.Handle("POST /mcp", mcp.NewHandler(s.st, s.version))
	mux.Handle("GET /", uiRoot(s.basePath))
	// One strip at the edge: everything above is written root-absolute, and a
	// request outside the prefix is a 404 rather than another app's SPA. The
	// guard and the response headers sit inside the strip so they see the same
	// root-relative paths the mux does.
	return basepath.Strip(s.basePath, securityHeaders(s.guard(mux)))
}

// uiRoot serves the embedded frontend with SPA fallback, or the placeholder
// page when the binary was built without `npm run build`. The handler sees
// root-relative paths: basepath.Strip removed the mount prefix already.
func uiRoot(basePath string) http.Handler {
	fsys, ok := web.Dist()
	if !ok {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			w.Write([]byte(placeholderHTML))
		})
	}
	fileServer := http.FileServerFS(fsys)
	index, err := indexHTML(fsys, basePath)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		p := strings.TrimPrefix(r.URL.Path, "/")
		if p != "" {
			if _, err := fs.Stat(fsys, p); err == nil {
				fileServer.ServeHTTP(w, r)
				return
			}
		}
		// Unknown paths get index.html — client-side routing owns them.
		if err != nil {
			http.ServeFileFS(w, r, fsys, "index.html")
			return
		}
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write(index)
	})
}

// indexHTML reads the built index.html with asset URLs pointed at basePath.
func indexHTML(fsys fs.FS, basePath string) ([]byte, error) {
	raw, err := fs.ReadFile(fsys, "index.html")
	if err != nil {
		return nil, err
	}
	return []byte(rewriteAssetRefs(string(raw), basePath)), nil
}

// rewriteAssetRefs prefixes the root-absolute href/src URLs in the built
// index.html with basePath. Vite already does this when the UI is compiled with
// OTTERSCOPE_BASE_PATH set; doing it here as well keeps a binary built at root
// usable under a prefix, where a stray "/assets/..." would otherwise fetch from
// the ingress root. Rewriting is idempotent, so a prefixed build passes through.
func rewriteAssetRefs(html, basePath string) string {
	if basePath == "" {
		return html
	}
	return assetRef.ReplaceAllStringFunc(html, func(m string) string {
		parts := assetRef.FindStringSubmatch(m) // [all, attr, ref]
		attr, ref := parts[1], parts[2]
		if ref == basePath || strings.HasPrefix(ref, basePath+"/") {
			return m
		}
		return attr + `="` + basePath + ref + `"`
	})
}

// assetRef matches a root-absolute href/src in built HTML, capturing the
// attribute name and the path (which starts with "/") separately.
var assetRef = regexp.MustCompile(`(href|src)="(/[^"]*)"`)

// tokenScopeKey carries the authenticated token's capability from authWrap to
// the route guards. A request has no scope when -read-auth is off.
type tokenScopeKey struct{}

// adminOnly guards a mutating route: with read auth on, the request must carry
// an admin token. With read auth off the whole API is open as before, so the
// guard is a no-op.
func (s *Server) adminOnly(h http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !s.readAuth {
			h(w, r)
			return
		}
		if scope, _ := r.Context().Value(tokenScopeKey{}).(store.Scope); scope != store.ScopeAdmin {
			writeJSON(w, http.StatusForbidden, map[string]string{"error": "admin token required"})
			return
		}
		h(w, r)
	}
}

// authWrap enforces read-token auth on the API + MCP when -read-auth is set.
// The static UI, health check, and public share endpoint stay open; the UI's
// own API calls carry a token (see the frontend apiFetch wrapper).
func (s *Server) authWrap(h http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Outside the mount nothing is served at all, so answering 401 there
		// would advertise an API the deployment does not expose.
		if _, inside := s.mountPath(r.URL.Path); !inside {
			h.ServeHTTP(w, r)
			return
		}
		if !s.readAuth || !s.authProtected(r.URL.Path) {
			h.ServeHTTP(w, r)
			return
		}
		token := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
		if token == "" {
			// SSE (EventSource) can't set headers; accept a query token.
			token = r.URL.Query().Get("token")
		}
		tok, ok := s.st.AuthenticateToken(r.Context(), token)
		if !ok {
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "read token required"})
			return
		}
		// Hand the capability to the guards below; the secret itself is not
		// needed past this point.
		r = r.WithContext(context.WithValue(r.Context(), tokenScopeKey{}, tok.Scope))
		h.ServeHTTP(w, r)
	})
}

// mountPath maps a request path (which still carries the mount prefix) to its
// root-relative form, reporting whether it is inside the mount at all.
func (s *Server) mountPath(p string) (string, bool) {
	if s.basePath == "" {
		return p, true
	}
	if p == s.basePath {
		return "/", true
	}
	if rest, ok := strings.CutPrefix(p, s.basePath); ok && strings.HasPrefix(rest, "/") {
		return rest, true
	}
	return "", false
}

// authProtected reports whether a path needs a read token under -read-auth.
func (s *Server) authProtected(path string) bool {
	p, _ := s.mountPath(path)
	// /healthz stays open so a liveness probe needs no credential. /metrics
	// does not: it is an inventory of the deployment (run counts, project
	// count, database size) and is exactly as sensitive as /api/stats, so it
	// takes the same read token. Public shares stay open by design.
	if p == "/healthz" || strings.HasPrefix(p, "/api/shared/") {
		return false
	}
	return strings.HasPrefix(p, "/api/") || p == "/mcp" || p == "/metrics"
}

func (s *Server) otlpHandler() http.Handler {
	// OTLP/HTTP exporters append /v1/traces to the endpoint they are given, so
	// the prefix applies to the receiver exactly as it does to the UI.
	o := s.ingestOpts
	o.BasePath = s.basePath
	// With read auth on, no request may reach "default" anonymously: every
	// project, the default one included, has to present an ingest key.
	o.RequireKey = s.readAuth
	return ingest.NewHandlerWithOptions(
		ingest.NewStoreSink(s.st, s.prices, s.eval, s.hub.broadcast), s.st.ProjectForKey, o)
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

// placeholderHTML is unstyled on purpose: the UI responses carry a strict CSP
// with no 'unsafe-inline', so a <style> block here would be ignored anyway.
const placeholderHTML = `<!doctype html>
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
</main>`
