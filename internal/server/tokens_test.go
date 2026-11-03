package server

import (
	"context"
	"database/sql"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/otterscope/otterscope/internal/evals"
	"github.com/otterscope/otterscope/internal/ingest"
	"github.com/otterscope/otterscope/internal/pricing"
	"github.com/otterscope/otterscope/internal/store"
)

// authServer is a server with read auth on, wired the way Run wires it:
// authWrap outside the UI mux, so the capability guards see the token's scope.
func authServer(t *testing.T) (*Server, *store.Store) {
	t.Helper()
	st, err := store.Open(context.Background(), filepath.Join(t.TempDir(), "auth.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	t.Cleanup(func() { st.Close() })
	return New(st, pricing.Default(), evals.Endpoint{}, 0, true, ingest.Options{}, "", "test"), st
}

// tokenReq sends a request through the same middleware chain production uses.
func tokenReq(t *testing.T, srv *Server, method, target, token, body string) *httptest.ResponseRecorder {
	t.Helper()
	var r *http.Request
	if body == "" {
		r = newReq(method, target, nil)
	} else {
		r = newReq(method, target, strings.NewReader(body))
		r.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		r.Header.Set("Authorization", "Bearer "+token)
	}
	w := httptest.NewRecorder()
	srv.authWrap(srv.uiHandler()).ServeHTTP(w, r)
	return w
}

// Every mutating route must refuse a read token. The old behaviour was worse
// than a data leak: a read token could mint itself an admin token.
func TestReadTokenCannotMutate(t *testing.T) {
	srv, st := authServer(t)
	ctx := context.Background()
	read, err := st.CreateToken(ctx, "reader", store.ScopeRead)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}
	admin, err := st.CreateToken(ctx, "operator", store.ScopeAdmin)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}

	mutations := []struct {
		method, path, body string
	}{
		{http.MethodPost, "/api/tokens", `{"name":"minted"}`},
		{http.MethodDelete, "/api/tokens/tok_nope", ""},
		{http.MethodPost, "/api/assertions", `{"project":"default","name":"a","type":"contains","config":"ok"}`},
		{http.MethodPost, "/api/assertions/evaluate", `{}`},
		{http.MethodDelete, "/api/assertions/1", ""},
		{http.MethodPost, "/api/alerts", `{"project":"default","name":"a","type":"error_rate","threshold":0.5}`},
		{http.MethodDelete, "/api/alerts/1", ""},
		{http.MethodPost, "/api/views", `{"name":"v","params":{}}`},
		{http.MethodDelete, "/api/views/1", ""},
	}
	for _, m := range mutations {
		if w := tokenReq(t, srv, m.method, m.path, read.Token, m.body); w.Code != http.StatusForbidden {
			t.Errorf("%s %s with a read token = %d, want 403 (body %s)",
				m.method, m.path, w.Code, strings.TrimSpace(w.Body.String()))
		}
	}

	// The admin token passes the capability gate; what the handler then makes
	// of the payload is not the gate's business.
	for _, m := range mutations {
		if w := tokenReq(t, srv, m.method, m.path, admin.Token, m.body); w.Code == http.StatusForbidden {
			t.Errorf("%s %s with an admin token = 403 (%s)", m.method, m.path, strings.TrimSpace(w.Body.String()))
		}
	}

	// Token inventory is admin-only too: it names every credential.
	if w := tokenReq(t, srv, http.MethodGet, "/api/tokens", read.Token, ""); w.Code != http.StatusForbidden {
		t.Errorf("GET /api/tokens with a read token = %d, want 403", w.Code)
	}
	// Reads stay open to a read token.
	for _, path := range []string{"/api/runs", "/api/stats", "/api/views", "/api/assertions", "/api/alerts"} {
		if w := tokenReq(t, srv, http.MethodGet, path, read.Token, ""); w.Code != http.StatusOK {
			t.Errorf("GET %s with a read token = %d, want 200", path, w.Code)
		}
	}
}

// A listing is an inventory, not a keyring: it may name a token but must not
// return anything that authenticates.
func TestTokenListingLeaksNoSecret(t *testing.T) {
	srv, st := authServer(t)
	ctx := context.Background()
	admin, err := st.CreateToken(ctx, "operator", store.ScopeAdmin)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}
	created, err := st.CreateToken(ctx, "ci", store.ScopeRead)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}

	w := tokenReq(t, srv, http.MethodGet, "/api/tokens", admin.Token, "")
	if w.Code != http.StatusOK {
		t.Fatalf("GET /api/tokens = %d (%s)", w.Code, w.Body.String())
	}
	body := w.Body.String()
	for _, secret := range []string{created.Token, admin.Token} {
		if strings.Contains(body, secret) {
			t.Fatalf("listing contains a usable credential: %s", body)
		}
	}

	var resp struct {
		Tokens []map[string]any `json:"tokens"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	if len(resp.Tokens) != 2 {
		t.Fatalf("got %d tokens, want 2", len(resp.Tokens))
	}
	var found bool
	for _, tok := range resp.Tokens {
		if _, ok := tok["token"]; ok {
			t.Fatalf("listing has a token field: %v", tok)
		}
		if tok["name"] != "ci" {
			continue
		}
		found = true
		if tok["id"] != created.ID {
			t.Errorf("id = %v, want %s", tok["id"], created.ID)
		}
		if tok["scope"] != "read" {
			t.Errorf("scope = %v, want read", tok["scope"])
		}
		if _, ok := tok["createdAt"]; !ok {
			t.Error("createdAt missing")
		}
	}
	if !found {
		t.Fatalf("created token not in listing: %v", resp.Tokens)
	}
}

// The secret exists in exactly one response: the one that mints it.
func TestCreateTokenResponseCarriesTheSecret(t *testing.T) {
	srv, st := authServer(t)
	ctx := context.Background()
	admin, err := st.CreateToken(ctx, "operator", store.ScopeAdmin)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}

	w := tokenReq(t, srv, http.MethodPost, "/api/tokens", admin.Token, `{"name":"ci"}`)
	if w.Code != http.StatusCreated {
		t.Fatalf("create = %d (%s)", w.Code, w.Body.String())
	}
	var created struct {
		ID        string      `json:"id"`
		Token     string      `json:"token"`
		Name      string      `json:"name"`
		Scope     store.Scope `json:"scope"`
		CreatedAt time.Time   `json:"createdAt"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &created); err != nil {
		t.Fatalf("bad JSON: %v", err)
	}
	if created.Token == "" || created.ID == "" || created.ID == created.Token {
		t.Fatalf("create response = %+v, want a secret and a distinct id", created)
	}
	if created.Scope != store.ScopeRead {
		t.Errorf("default scope = %q, want read", created.Scope)
	}

	// The returned secret works, as a read token.
	if w := tokenReq(t, srv, http.MethodGet, "/api/runs", created.Token, ""); w.Code != http.StatusOK {
		t.Errorf("GET /api/runs with the new token = %d, want 200", w.Code)
	}
	if w := tokenReq(t, srv, http.MethodPost, "/api/views", created.Token, `{"name":"v","params":{}}`); w.Code != http.StatusForbidden {
		t.Errorf("mutation with the new read token = %d, want 403", w.Code)
	}
	// And it is not recoverable afterwards.
	list := tokenReq(t, srv, http.MethodGet, "/api/tokens", admin.Token, "")
	if strings.Contains(list.Body.String(), created.Token) {
		t.Error("the created secret is recoverable from the listing")
	}
}

// Revoking by id (the only identifier the API exposes) removes the credential.
func TestDeleteTokenByIDRevokes(t *testing.T) {
	srv, st := authServer(t)
	ctx := context.Background()
	admin, err := st.CreateToken(ctx, "operator", store.ScopeAdmin)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}
	victim, err := st.CreateToken(ctx, "old-ci", store.ScopeRead)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}

	if w := tokenReq(t, srv, http.MethodDelete, "/api/tokens/"+victim.ID, admin.Token, ""); w.Code != http.StatusNoContent {
		t.Fatalf("delete = %d (%s)", w.Code, w.Body.String())
	}
	if w := tokenReq(t, srv, http.MethodGet, "/api/runs", victim.Token, ""); w.Code != http.StatusUnauthorized {
		t.Errorf("revoked token = %d, want 401", w.Code)
	}
	if w := tokenReq(t, srv, http.MethodDelete, "/api/tokens/"+victim.ID, admin.Token, ""); w.Code != http.StatusNotFound {
		t.Errorf("second delete = %d, want 404", w.Code)
	}
}

// An operator can see which tokens are actually in use.
func TestTokenLastUsedOverAPI(t *testing.T) {
	srv, st := authServer(t)
	ctx := context.Background()
	admin, err := st.CreateToken(ctx, "operator", store.ScopeAdmin)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}
	read, err := st.CreateToken(ctx, "dashboard", store.ScopeRead)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}

	list := func() []store.Token {
		t.Helper()
		w := tokenReq(t, srv, http.MethodGet, "/api/tokens", admin.Token, "")
		if w.Code != http.StatusOK {
			t.Fatalf("list = %d (%s)", w.Code, w.Body.String())
		}
		var resp struct {
			Tokens []store.Token `json:"tokens"`
		}
		if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
			t.Fatalf("bad JSON: %v", err)
		}
		return resp.Tokens
	}

	for _, tok := range list() {
		if tok.ID == read.ID && tok.LastUsedAt != nil {
			t.Fatalf("lastUsedAt set before use: %v", tok.LastUsedAt)
		}
	}
	if w := tokenReq(t, srv, http.MethodGet, "/api/runs", read.Token, ""); w.Code != http.StatusOK {
		t.Fatalf("GET /api/runs = %d", w.Code)
	}
	for _, tok := range list() {
		if tok.ID != read.ID {
			continue
		}
		if tok.LastUsedAt == nil {
			t.Fatal("lastUsedAt still empty after use")
		}
		if time.Since(*tok.LastUsedAt) > time.Minute {
			t.Errorf("lastUsedAt = %v, want just now", tok.LastUsedAt)
		}
		return
	}
	t.Fatal("read token missing from listing")
}

// A token column predating scopes must keep working after the upgrade and must
// not gain mutation rights it never had.
func TestPreMigrationTokenReadsButCannotMutate(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "legacy.db")

	st, err := store.Open(ctx, path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := st.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	// Rewind read_tokens to the 0017 shape and forget that 0021 ran.
	db, err := sql.Open("sqlite", "file:"+path)
	if err != nil {
		t.Fatalf("open raw: %v", err)
	}
	legacy := "fedcba9876543210fedcba9876543210"
	for _, q := range []string{
		`DROP TABLE read_tokens`,
		`CREATE TABLE read_tokens (token TEXT PRIMARY KEY, name TEXT NOT NULL, created_ns INTEGER NOT NULL)`,
	} {
		if _, err := db.Exec(q); err != nil {
			t.Fatalf("rewind: %v", err)
		}
	}
	if _, err := db.Exec(`INSERT INTO read_tokens (token, name, created_ns) VALUES (?,?,?)`,
		legacy, "old-dashboard", time.Now().Add(-time.Hour).UnixNano()); err != nil {
		t.Fatalf("insert legacy: %v", err)
	}
	if _, err := db.Exec(`DELETE FROM schema_migrations WHERE name LIKE '%0021_token_scopes.sql'`); err != nil {
		t.Fatalf("forget migration: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("close raw: %v", err)
	}

	st2, err := store.Open(ctx, path)
	if err != nil {
		t.Fatalf("reopen (migrate): %v", err)
	}
	t.Cleanup(func() { st2.Close() })
	srv := New(st2, pricing.Default(), evals.Endpoint{}, 0, true, ingest.Options{}, "", "test")

	if w := tokenReq(t, srv, http.MethodGet, "/api/runs", legacy, ""); w.Code != http.StatusOK {
		t.Fatalf("legacy token read = %d, want 200 (%s)", w.Code, w.Body.String())
	}
	if w := tokenReq(t, srv, http.MethodPost, "/api/views", legacy, `{"name":"v","params":{}}`); w.Code != http.StatusForbidden {
		t.Errorf("legacy token mutation = %d, want 403", w.Code)
	}
	if w := tokenReq(t, srv, http.MethodPost, "/api/tokens", legacy, `{"name":"minted"}`); w.Code != http.StatusForbidden {
		t.Errorf("legacy token mint = %d, want 403", w.Code)
	}
}
