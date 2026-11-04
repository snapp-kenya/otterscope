//! On-disk footprint: private file modes, SQLite backups and the snapshot
//! taken before an upgrade touches an existing database.

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use rusqlite::Connection;

use super::{is_memory_path, now_ns};

const DB_FILE_MODE: u32 = 0o600;
const DB_DIR_MODE: u32 = 0o700;
const MAX_PRE_MIGRATION_BACKUPS: usize = 1;

/// Prepare the on-disk footprint before SQLite opens it: create the parent
/// directory 0700, create the database 0600, and tighten the WAL/SHM if they
/// already exist.
pub(crate) fn secure_open(path: &str) -> anyhow::Result<()> {
    if is_memory_path(path) {
        return Ok(());
    }
    if let Some(dir) = Path::new(path).parent()
        && !dir.as_os_str().is_empty()
        && dir != Path::new(".")
    {
        let fresh = !dir.exists();
        fs::create_dir_all(dir)
            .with_context(|| format!("create database directory {}", dir.display()))?;
        if let Err(err) = tighten_dir(dir) {
            if fresh {
                return Err(err);
            }
            // An existing directory may be a shared mount we do not own; the
            // database file itself is still locked down below.
            tracing::warn!(dir = %dir.display(), error = %err, "could not tighten database directory permissions");
        }
    }

    let mut opts = OpenOptions::new();
    opts.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(DB_FILE_MODE);
    }
    opts.open(path)
        .with_context(|| format!("create database {path}"))?;
    restrict_perms(path)
}

/// Force a store-managed directory to 0700, leaving shared locations (the
/// filesystem root, the system temp dir and the user's home) alone.
pub(crate) fn tighten_dir(dir: &Path) -> anyhow::Result<()> {
    if dir == Path::new("/") || dir == std::env::temp_dir() {
        return Ok(());
    }
    if let Some(home) = std::env::var_os("HOME")
        && dir == Path::new(&home)
    {
        return Ok(());
    }
    set_file_mode(dir, DB_DIR_MODE)
        .with_context(|| format!("restrict database directory {}", dir.display()))
}

/// Make the database and its WAL/SHM siblings owner-only. Missing siblings are
/// normal: SQLite creates them on the first write and removes them on close.
pub(crate) fn restrict_perms(path: &str) -> anyhow::Result<()> {
    if is_memory_path(path) {
        return Ok(());
    }
    for p in [
        path.to_string(),
        format!("{path}-wal"),
        format!("{path}-shm"),
    ] {
        match set_file_mode(Path::new(&p), DB_FILE_MODE) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("restrict {p}")),
        }
    }
    Ok(())
}

fn set_file_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

pub(crate) fn backup_conn(conn: &Connection, dest: &str) -> anyhow::Result<()> {
    conn.execute("VACUUM INTO ?1", [dest])
        .with_context(|| format!("backup to {dest}"))?;
    match set_file_mode(Path::new(dest), DB_FILE_MODE) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("backup to {dest}")),
    }
}

/// Snapshot the database next to itself before an upgrade. Called while the
/// connection lock is held, so it must not re-enter [`super::Store::lock`].
pub(crate) fn backup_before_migrate(conn: &Connection, db_path: &str) -> anyhow::Result<()> {
    if is_memory_path(db_path) {
        return Ok(());
    }
    let dest = format!("{db_path}.pre-migrate-{}.bak", now_ns());
    backup_conn(conn, &dest)?;
    // Prune only after the new snapshot exists, so a pruning bug can never
    // leave the upgrade with no backup at all.
    if let Err(err) = prune_pre_migration_backups(db_path, MAX_PRE_MIGRATION_BACKUPS) {
        tracing::warn!(error = %err, "could not prune old pre-migration backups");
    }
    tracing::info!(backup = %dest, "backed up database before applying migrations");
    Ok(())
}

fn prune_pre_migration_backups(db_path: &str, keep: usize) -> anyhow::Result<()> {
    let db = Path::new(db_path);
    let dir = match db.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let file_name = db.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let prefix = format!("{file_name}.pre-migrate-");

    let mut backups: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(&prefix) || !name.ends_with(".bak") {
            continue;
        }
        let path = entry.path();
        match entry.metadata() {
            Ok(meta) => {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                backups.push((path, modified));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        }
    }
    if backups.len() <= keep {
        return Ok(());
    }
    // Newest first; the nanosecond-stamped name breaks mtime ties.
    backups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    for (path, _) in backups.into_iter().skip(keep) {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("remove old backup {}", path.display()));
            }
        }
    }
    Ok(())
}

impl super::Store {
    /// Write a consistent snapshot of the database to `dest` using SQLite's
    /// VACUUM INTO (includes committed WAL). `dest` must not already exist.
    pub fn backup(&self, dest: &str) -> anyhow::Result<()> {
        let conn = self.lock()?;
        backup_conn(&conn, dest)
    }

    #[cfg(test)]
    pub(crate) fn backup_before_migrate(&self) -> anyhow::Result<()> {
        let conn = self.lock()?;
        backup_before_migrate(&conn, &self.path)
    }
}
