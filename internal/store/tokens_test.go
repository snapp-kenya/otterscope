package store

import (
	"context"
	"database/sql"
	"errors"
	"path/filepath"
	"testing"
	"time"
)

// A token is a credential: the database must hold a digest, not the token. If
// the file (or a backup) leaks, nothing in it can be replayed against the API.
func TestCreateTokenStoresOnlyDigest(t *testing.T) {
	ctx := context.Background()
	st, err := Open(ctx, filepath.Join(t.TempDir(), "t.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	tok, err := st.CreateToken(ctx, "ci", ScopeRead)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}
	if len(tok.Token) != 32 {
		t.Fatalf("token = %q, want the existing 32-hex-char format", tok.Token)
	}

	var id, hash, scope string
	if err := st.reader.QueryRowContext(ctx,
		`SELECT id, token_hash, scope FROM read_tokens WHERE id = ?`, tok.ID).
		Scan(&id, &hash, &scope); err != nil {
		t.Fatalf("read row: %v", err)
	}
	if id != tok.ID {
		t.Errorf("id = %q, want %q", id, tok.ID)
	}
	if scope != string(ScopeRead) {
		t.Errorf("scope = %q, want read", scope)
	}
	if hash != tokenHash(tok.Token) {
		t.Errorf("token_hash is not sha256(token)")
	}
	if hash == tok.Token {
		t.Errorf("the token itself was stored")
	}
	// And the secret appears in no column of the table.
	var n int
	if err := st.reader.QueryRowContext(ctx,
		`SELECT count(*) FROM read_tokens WHERE id = ? OR token_hash = ? OR name = ?`,
		tok.Token, tok.Token, tok.Token).Scan(&n); err != nil {
		t.Fatalf("scan for secret: %v", err)
	}
	if n != 0 {
		t.Errorf("the secret is still stored in %d column(s)", n)
	}
}

func TestAuthenticateToken(t *testing.T) {
	ctx := context.Background()
	st, err := Open(ctx, filepath.Join(t.TempDir(), "t.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	read, err := st.CreateReadToken(ctx, "script")
	if err != nil {
		t.Fatalf("CreateReadToken: %v", err)
	}
	admin, err := st.CreateToken(ctx, "operator", ScopeAdmin)
	if err != nil {
		t.Fatalf("CreateToken: %v", err)
	}

	got, ok := st.AuthenticateToken(ctx, read.Token)
	if !ok {
		t.Fatal("valid token rejected")
	}
	if got.ID != read.ID || got.Name != "script" || got.Scope != ScopeRead {
		t.Fatalf("authenticated %+v, want id=%s scope=read", got, read.ID)
	}
	if got, ok := st.AuthenticateToken(ctx, admin.Token); !ok || got.Scope != ScopeAdmin {
		t.Fatalf("admin token: ok=%v scope=%q", ok, got.Scope)
	}
	if _, ok := st.AuthenticateToken(ctx, "nope"); ok {
		t.Error("unknown token accepted")
	}
	if _, ok := st.AuthenticateToken(ctx, ""); ok {
		t.Error("empty token accepted")
	}
	if _, ok := st.AuthenticateToken(ctx, read.Token+"0"); ok {
		t.Error("token with trailing junk accepted")
	}
}

// An operator needs to tell a live token from one nobody uses; a successful
// authentication records the use, a listing exposes it.
func TestLastUsedIsRecorded(t *testing.T) {
	ctx := context.Background()
	st, err := Open(ctx, filepath.Join(t.TempDir(), "t.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	tok, err := st.CreateReadToken(ctx, "dashboard")
	if err != nil {
		t.Fatalf("CreateReadToken: %v", err)
	}
	if list, err := st.ListReadTokens(ctx); err != nil {
		t.Fatalf("ListReadTokens: %v", err)
	} else if list[0].LastUsedAt != nil {
		t.Fatalf("last_used set before any use: %v", list[0].LastUsedAt)
	}

	before := time.Now().Add(-time.Second)
	if _, ok := st.AuthenticateToken(ctx, tok.Token); !ok {
		t.Fatal("authenticate failed")
	}
	list, err := st.ListReadTokens(ctx)
	if err != nil {
		t.Fatalf("ListReadTokens: %v", err)
	}
	if len(list) != 1 || list[0].LastUsedAt == nil {
		t.Fatalf("last_used not recorded: %+v", list)
	}
	if list[0].LastUsedAt.Before(before) {
		t.Errorf("last_used = %v, want recent", list[0].LastUsedAt)
	}
}

func TestDeleteTokenByID(t *testing.T) {
	ctx := context.Background()
	st, err := Open(ctx, filepath.Join(t.TempDir(), "t.db"))
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	defer st.Close()

	tok, err := st.CreateReadToken(ctx, "gone")
	if err != nil {
		t.Fatalf("CreateReadToken: %v", err)
	}
	if err := st.DeleteReadToken(ctx, tok.ID); err != nil {
		t.Fatalf("DeleteReadToken: %v", err)
	}
	if _, ok := st.AuthenticateToken(ctx, tok.Token); ok {
		t.Error("revoked token still authenticates")
	}
	if err := st.DeleteReadToken(ctx, tok.ID); !errors.Is(err, ErrNotFound) {
		t.Errorf("second delete = %v, want ErrNotFound", err)
	}
}

// A token minted before migration 0021 kept its secret in the primary key.
// Opening the database must hash it, retire the plaintext, and leave it
// working as a read token — never more.
func TestLegacyTokenMigratesToReadOnly(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "legacy.db")

	// Make a current database, then rewind read_tokens to its pre-0021 shape
	// and forget that 0021 ran: that is exactly what an old file looks like.
	st, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := st.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	db, err := sql.Open("sqlite", "file:"+path)
	if err != nil {
		t.Fatalf("open raw: %v", err)
	}
	legacy := "0123456789abcdef0123456789abcdef"
	created := time.Now().Add(-time.Hour)
	for _, q := range []string{
		`DROP TABLE read_tokens`,
		`CREATE TABLE read_tokens (token TEXT PRIMARY KEY, name TEXT NOT NULL, created_ns INTEGER NOT NULL)`,
	} {
		if _, err := db.Exec(q); err != nil {
			t.Fatalf("rewind: %v", err)
		}
	}
	if _, err := db.Exec(`INSERT INTO read_tokens (token, name, created_ns) VALUES (?,?,?)`,
		legacy, "old-dashboard", created.UnixNano()); err != nil {
		t.Fatalf("insert legacy: %v", err)
	}
	if _, err := db.Exec(`DELETE FROM schema_migrations WHERE name LIKE '%0021_token_scopes.sql'`); err != nil {
		t.Fatalf("forget migration: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Fatalf("close raw: %v", err)
	}

	st2, err := Open(ctx, path)
	if err != nil {
		t.Fatalf("reopen (migrate): %v", err)
	}
	defer st2.Close()

	tok, ok := st2.AuthenticateToken(ctx, legacy)
	if !ok {
		t.Fatal("a token created before the migration no longer authenticates")
	}
	if tok.Scope != ScopeRead {
		t.Errorf("legacy scope = %q, want read", tok.Scope)
	}
	if tok.ID == legacy || tok.ID == "" {
		t.Errorf("id = %q, want a fresh identifier", tok.ID)
	}

	var id, hash string
	if err := st2.reader.QueryRowContext(ctx,
		`SELECT id, token_hash FROM read_tokens`).Scan(&id, &hash); err != nil {
		t.Fatalf("read migrated row: %v", err)
	}
	if id == legacy {
		t.Error("the plaintext token is still the primary key")
	}
	if hash != tokenHash(legacy) {
		t.Errorf("token_hash = %q, want sha256 of the old token", hash)
	}
}
