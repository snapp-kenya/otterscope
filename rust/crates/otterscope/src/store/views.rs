//! Named, saved filter/search combinations ("my dashboards").

use rusqlite::params;

use super::{SavedView, now_ns};

impl super::Store {
    /// Store a view. `params` is opaque JSON (the UI's filter state).
    pub fn create_saved_view(
        &self,
        name: &str,
        params: &serde_json::Value,
    ) -> anyhow::Result<SavedView> {
        let text = serde_json::to_string(params)?;
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO saved_views (name, params_json, created_ns) VALUES (?,?,?)",
            params![name, text, now_ns()],
        )
        .map_err(super::classify_write)?;
        let view = SavedView {
            id: conn.last_insert_rowid(),
            name: name.to_string(),
            params: params.clone(),
        };
        super::audit::record(&conn, "create", "view", name);
        Ok(view)
    }

    /// All views, oldest first.
    pub fn list_saved_views(&self) -> anyhow::Result<Vec<SavedView>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT id, name, params_json FROM saved_views ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            let text: String = row.get(2)?;
            Ok(SavedView {
                id: row.get(0)?,
                name: row.get(1)?,
                params: serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
            })
        })?;
        let mut views = Vec::new();
        for row in rows {
            views.push(row?);
        }
        Ok(views)
    }

    /// Remove a view. False means there was nothing to remove.
    pub fn delete_saved_view(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.lock()?;
        if conn.execute("DELETE FROM saved_views WHERE id = ?", params![id])? == 0 {
            return Ok(false);
        }
        super::audit::record(&conn, "delete", "view", &id.to_string());
        Ok(true)
    }
}
