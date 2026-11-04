//! Bearer tokens: the secret is returned once and only its SHA-256 digest is
//! persisted.

use anyhow::Context;
use chrono::{Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::{Scope, Token, datetime, nanos, parse_scope, random_hex};

/// Bounds how often an authenticated request writes `last_used_ns`: often
/// enough to see which tokens are live, without a write on every poll.
const TOKEN_TOUCH_INTERVAL: Duration = Duration::minutes(1);

impl super::Store {
    /// Mint a named read-only token.
    pub fn create_read_token(&self, name: &str) -> anyhow::Result<super::CreatedToken> {
        self.create_token(name, Scope::Read)
    }

    /// Mint a named token with scope and return the secret once. Only the
    /// digest is persisted, so the value cannot be recovered later.
    pub fn create_token(&self, name: &str, scope: Scope) -> anyhow::Result<super::CreatedToken> {
        let scope = match scope {
            Scope::Admin => Scope::Admin,
            _ => Scope::Read, // unknown or empty scope fails closed
        };
        let secret = random_hex(16)?; // 32 hex chars: the format clients hold
        let id = new_token_id()?;
        let created_at = Utc::now();
        let token = super::CreatedToken {
            id,
            token: secret,
            name: name.to_string(),
            scope,
            created_at,
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO read_tokens (id, token_hash, name, scope, created_ns) VALUES (?,?,?,?,?)",
            params![
                token.id,
                token_hash(&token.token),
                token.name,
                token.scope.as_str(),
                nanos(token.created_at)
            ],
        )
        .map_err(super::classify_write)?;
        super::audit::record(&conn, "create", "token", name);
        Ok(token)
    }

    /// Every token's identity, oldest first. The secret is unreachable: only
    /// its irreversible digest is stored.
    pub fn list_tokens(&self) -> anyhow::Result<Vec<Token>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, scope, created_ns, last_used_ns FROM read_tokens ORDER BY created_ns",
        )?;
        let rows = stmt.query_map([], |row| {
            let scope: String = row.get(2)?;
            let created_ns: i64 = row.get(3)?;
            let used_ns: Option<i64> = row.get(4)?;
            Ok(Token {
                id: row.get(0)?,
                name: row.get(1)?,
                scope: parse_scope(&scope),
                created_at: datetime(created_ns),
                last_used_at: used_ns.map(datetime),
            })
        })?;
        let mut tokens = Vec::new();
        for row in rows {
            tokens.push(row?);
        }
        Ok(tokens)
    }

    /// Revoke the token with this id (never the secret). False means there
    /// was nothing to revoke.
    pub fn delete_token(&self, id: &str) -> anyhow::Result<bool> {
        let conn = self.lock()?;
        if conn.execute("DELETE FROM read_tokens WHERE id = ?", params![id])? == 0 {
            return Ok(false);
        }
        super::audit::record(&conn, "delete", "token", &super::short_token(id));
        Ok(true)
    }

    /// Resolve a presented bearer token to its record. The comparison is an
    /// indexed digest lookup, so a stolen database yields no usable token.
    pub fn authenticate_token(&self, presented: &str) -> anyhow::Result<Option<Token>> {
        if presented.is_empty() {
            return Ok(None);
        }
        let conn = self.lock()?;
        let token = conn
            .query_row(
                "SELECT id, name, scope, created_ns, last_used_ns FROM read_tokens WHERE token_hash = ?",
                params![token_hash(presented)],
                |row| {
                    let scope: String = row.get(2)?;
                    let created_ns: i64 = row.get(3)?;
                    let used_ns: Option<i64> = row.get(4)?;
                    Ok(Token {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        scope: parse_scope(&scope),
                        created_at: datetime(created_ns),
                        last_used_at: used_ns.map(datetime),
                    })
                },
            )
            .optional()?;
        let Some(mut token) = token else {
            return Ok(None);
        };

        let now = Utc::now();
        let stale = token
            .last_used_at
            .is_none_or(|last| now - last >= TOKEN_TOUCH_INTERVAL);
        if stale {
            // Best effort: a failed bookkeeping write must not turn a valid
            // token into an authentication failure.
            if conn
                .execute(
                    "UPDATE read_tokens SET last_used_ns = ? WHERE id = ?",
                    params![nanos(now), token.id],
                )
                .is_ok()
            {
                token.last_used_at = Some(now);
            }
        }
        Ok(Some(token))
    }
}

/// Rewrite pre-0021 rows, which kept the secret in the primary key. Rows
/// already hashed are skipped, so this is safe on every open.
pub(crate) fn hash_legacy(conn: &Connection) -> anyhow::Result<()> {
    let legacy = {
        let mut stmt = conn.prepare("SELECT id FROM read_tokens WHERE token_hash IS NULL")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        ids
    };
    for old in legacy {
        let id = new_token_id()?;
        // Guard on token_hash so a concurrent rewrite cannot re-hash an id.
        conn.execute(
            "UPDATE read_tokens SET id = ?, token_hash = ? WHERE id = ? AND token_hash IS NULL",
            params![id, token_hash(&old), old],
        )
        .with_context(|| format!("migrate legacy token {old}"))?;
    }
    Ok(())
}

pub(crate) fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    super::hex(&digest)
}

fn new_token_id() -> anyhow::Result<String> {
    Ok(format!("tok_{}", random_hex(8)?))
}
