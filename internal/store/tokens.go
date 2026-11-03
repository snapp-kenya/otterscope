package store

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"time"
)

// Scope is what a bearer token may do. A read token can query the API; an
// admin token can also mutate it: mint and revoke tokens, write assertions,
// delete runs, create views and alerts.
type Scope string

const (
	ScopeRead  Scope = "read"
	ScopeAdmin Scope = "admin"
)

// Token is a token's identity: id, name, scope and timestamps. It never
// carries the secret, so it is safe to return from a listing or print in the
// CLI.
type Token struct {
	ID         string     `json:"id"`
	Name       string     `json:"name"`
	Scope      Scope      `json:"scope"`
	CreatedAt  time.Time  `json:"createdAt"`
	LastUsedAt *time.Time `json:"lastUsedAt,omitempty"`
}

// CreatedToken is the response to minting a token, and the only place the
// secret is ever returned. The database holds nothing but its SHA-256.
type CreatedToken struct {
	ID        string    `json:"id"`
	Token     string    `json:"token"`
	Name      string    `json:"name"`
	Scope     Scope     `json:"scope"`
	CreatedAt time.Time `json:"createdAt"`
}

// tokenTouchInterval bounds how often an authenticated request writes
// last_used_ns: often enough for an operator to see which tokens are live,
// without turning every dashboard poll into a write on the single writer
// connection.
const tokenTouchInterval = time.Minute

// CreateReadToken mints a named read-only token.
func (s *Store) CreateReadToken(ctx context.Context, name string) (CreatedToken, error) {
	return s.CreateToken(ctx, name, ScopeRead)
}

// CreateToken mints a named token with scope and returns the secret once.
// Only the digest is persisted, so the value cannot be recovered later.
func (s *Store) CreateToken(ctx context.Context, name string, scope Scope) (CreatedToken, error) {
	if scope != ScopeAdmin {
		scope = ScopeRead // unknown or empty scope fails closed
	}
	secret, err := randomHex(16) // 32 hex chars: the format clients already hold
	if err != nil {
		return CreatedToken{}, err
	}
	id, err := newTokenID()
	if err != nil {
		return CreatedToken{}, err
	}
	t := CreatedToken{
		ID: id, Token: secret, Name: name,
		Scope: scope, CreatedAt: time.Now().UTC(),
	}
	_, err = s.writer.ExecContext(ctx,
		`INSERT INTO read_tokens (id, token_hash, name, scope, created_ns) VALUES (?,?,?,?,?)`,
		t.ID, tokenHash(t.Token), t.Name, string(t.Scope), t.CreatedAt.UnixNano())
	if err != nil {
		return CreatedToken{}, err
	}
	s.audit(ctx, "create", "token", name)
	return t, nil
}

// ListReadTokens returns every token's identity, oldest first, with no way to
// reach the credential: the use is at most the digest, and the digest cannot
// be reversed.
func (s *Store) ListReadTokens(ctx context.Context) ([]Token, error) {
	rows, err := s.reader.QueryContext(ctx,
		`SELECT id, name, scope, created_ns, last_used_ns FROM read_tokens ORDER BY created_ns`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Token
	for rows.Next() {
		var t Token
		var scope string
		var createdNS int64
		var usedNS *int64
		if err := rows.Scan(&t.ID, &t.Name, &scope, &createdNS, &usedNS); err != nil {
			return nil, err
		}
		t.Scope = parseScope(scope)
		t.CreatedAt = time.Unix(0, createdNS).UTC()
		if usedNS != nil {
			used := time.Unix(0, *usedNS).UTC()
			t.LastUsedAt = &used
		}
		out = append(out, t)
	}
	return out, rows.Err()
}

// DeleteReadToken revokes the token with this id (the identifier from
// ListReadTokens, never the secret).
func (s *Store) DeleteReadToken(ctx context.Context, id string) error {
	res, err := s.writer.ExecContext(ctx, `DELETE FROM read_tokens WHERE id = ?`, id)
	if err != nil {
		return err
	}
	if err := deleted(res); err != nil {
		return err
	}
	s.audit(ctx, "delete", "token", shortToken(id))
	return nil
}

// AuthenticateToken resolves a presented bearer token to its record, or
// reports false. The comparison is a digest lookup on an indexed column, so a
// stolen database yields no usable token. A successful authentication records
// last use (at most once per tokenTouchInterval).
func (s *Store) AuthenticateToken(ctx context.Context, presented string) (Token, bool) {
	if presented == "" {
		return Token{}, false
	}
	var t Token
	var scope string
	var createdNS int64
	var usedNS *int64
	err := s.reader.QueryRowContext(ctx,
		`SELECT id, name, scope, created_ns, last_used_ns FROM read_tokens WHERE token_hash = ?`,
		tokenHash(presented)).
		Scan(&t.ID, &t.Name, &scope, &createdNS, &usedNS)
	if err != nil {
		return Token{}, false
	}
	t.Scope = parseScope(scope)
	t.CreatedAt = time.Unix(0, createdNS).UTC()
	if usedNS != nil {
		used := time.Unix(0, *usedNS).UTC()
		t.LastUsedAt = &used
	}
	now := time.Now().UTC()
	if t.LastUsedAt == nil || now.Sub(*t.LastUsedAt) >= tokenTouchInterval {
		// Best effort: a failed bookkeeping write must not turn a valid token
		// into a 401.
		if _, err := s.writer.ExecContext(ctx,
			`UPDATE read_tokens SET last_used_ns = ? WHERE id = ?`, now.UnixNano(), t.ID); err == nil {
			t.LastUsedAt = &now
		}
	}
	return t, true
}

// hashLegacyTokens rewrites rows created before migration 0021, which kept the
// secret in the primary key: the digest is stored and the id replaced with an
// opaque one, so no plaintext credential survives an upgrade. Rows already
// hashed are skipped, making it safe on every open.
func (s *Store) hashLegacyTokens(ctx context.Context) error {
	rows, err := s.writer.QueryContext(ctx,
		`SELECT id FROM read_tokens WHERE token_hash IS NULL`)
	if err != nil {
		return err
	}
	var legacy []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return err
		}
		legacy = append(legacy, id)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return err
	}
	for _, old := range legacy {
		id, err := newTokenID()
		if err != nil {
			return err
		}
		// Guard on token_hash so a concurrent rewrite cannot re-hash an id.
		if _, err := s.writer.ExecContext(ctx,
			`UPDATE read_tokens SET id = ?, token_hash = ? WHERE id = ? AND token_hash IS NULL`,
			id, tokenHash(old), old); err != nil {
			return err
		}
	}
	return nil
}

// parseScope narrows anything unexpected to read: a bad value in the database
// must not grant mutation rights.
func parseScope(s string) Scope {
	if Scope(s) == ScopeAdmin {
		return ScopeAdmin
	}
	return ScopeRead
}

// tokenHash is the digest stored in place of a token. A token is 128 bits of
// kernel randomness, so SHA-256 is enough: there is no guessable dictionary,
// only 2^128 candidates.
func tokenHash(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

func randomHex(n int) (string, error) {
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return hex.EncodeToString(buf), nil
}

// newTokenID returns a public identifier. It is prefixed so it can never be
// mistaken for a credential (tokens are bare 32 hex chars).
func newTokenID() (string, error) {
	id, err := randomHex(8)
	if err != nil {
		return "", err
	}
	return "tok_" + id, nil
}
