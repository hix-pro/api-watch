use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::diff::Change;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone)]
pub struct Target {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub created_at: String,
    pub last_checked_at: Option<String>,
    pub last_status: Option<String>,
    pub last_breaking: i64,
    pub last_compatible: i64,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: i64,
    pub target_id: i64,
    pub captured_at: String,
    pub format: String,
    pub etag: Option<String>,
    pub content: String,
    pub fact_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: i64,
    pub checked_at: String,
    pub status: String,
    pub http_status: Option<i64>,
    pub breaking: i64,
    pub compatible: i64,
    pub message: Option<String>,
    pub snapshot_id: Option<i64>,
}

pub struct NewSnapshot<'a> {
    pub captured_at: &'a str,
    pub format: &'a str,
    pub etag: Option<&'a str>,
    pub content: &'a str,
    pub fact_count: usize,
}

pub struct NewCheck<'a> {
    pub checked_at: &'a str,
    pub status: &'a str,
    pub http_status: Option<u16>,
    pub breaking: usize,
    pub compatible: usize,
    pub message: Option<&'a str>,
    pub snapshot: Option<NewSnapshot<'a>>,
    pub changes: &'a [Change],
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
            restrict_permissions(parent, 0o700);
        }

        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        restrict_permissions(path, 0o600);

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;

        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            bail!(
                "database schema version {version} is newer than this build supports ({SCHEMA_VERSION})"
            );
        }
        if version == 0 {
            init_schema(&conn)?;
        }

        Ok(Self { conn })
    }

    pub fn add_target(
        &self,
        name: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        now: &str,
    ) -> Result<Target> {
        if self.target(name)?.is_some() {
            bail!("target '{name}' already exists");
        }
        let headers_json = serde_json::to_string(headers)?;
        self.conn.execute(
            "INSERT INTO targets (name, url, headers, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![name, url, headers_json, now],
        )?;
        self.target(name)?
            .with_context(|| format!("target '{name}' disappeared after insert"))
    }

    pub fn target(&self, name: &str) -> Result<Option<Target>> {
        self.conn
            .query_row(
                "SELECT id, name, url, headers, created_at, last_checked_at,
                        last_status, last_breaking, last_compatible
                 FROM targets WHERE name = ?1",
                [name],
                target_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn targets(&self) -> Result<Vec<Target>> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, url, headers, created_at, last_checked_at,
                    last_status, last_breaking, last_compatible
             FROM targets ORDER BY name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], target_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn remove_target(&self, name: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM targets WHERE name = ?1", [name])?;
        Ok(affected > 0)
    }

    pub fn latest_snapshot(&self, target_id: i64) -> Result<Option<Snapshot>> {
        self.conn
            .query_row(
                "SELECT id, target_id, captured_at, format, etag, content, fact_count
                 FROM snapshots WHERE target_id = ?1 ORDER BY id DESC LIMIT 1",
                [target_id],
                |row| {
                    Ok(Snapshot {
                        id: row.get(0)?,
                        target_id: row.get(1)?,
                        captured_at: row.get(2)?,
                        format: row.get(3)?,
                        etag: row.get(4)?,
                        content: row.get(5)?,
                        fact_count: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn record_check(&mut self, target_id: i64, check: NewCheck<'_>) -> Result<i64> {
        let transaction = self.conn.transaction()?;

        let snapshot_id = match check.snapshot {
            Some(snapshot) => {
                transaction.execute(
                    "INSERT INTO snapshots (target_id, captured_at, format, etag, content, fact_count)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        target_id,
                        snapshot.captured_at,
                        snapshot.format,
                        snapshot.etag,
                        snapshot.content,
                        snapshot.fact_count as i64
                    ],
                )?;
                Some(transaction.last_insert_rowid())
            }
            None => None,
        };

        transaction.execute(
            "INSERT INTO checks (
                target_id, checked_at, status, http_status, breaking, compatible, message, snapshot_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                target_id,
                check.checked_at,
                check.status,
                check.http_status.map(i64::from),
                check.breaking as i64,
                check.compatible as i64,
                check.message,
                snapshot_id
            ],
        )?;
        let check_id = transaction.last_insert_rowid();

        {
            let mut insert = transaction.prepare(
                "INSERT INTO changes (check_id, kind, area, label, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for change in check.changes {
                insert.execute(params![
                    check_id,
                    change.kind.label(),
                    area_name(change.area),
                    change.label,
                    change.detail
                ])?;
            }
        }

        transaction.execute(
            "UPDATE targets SET
                last_checked_at = ?1,
                last_status = ?2,
                last_breaking = ?3,
                last_compatible = ?4
             WHERE id = ?5",
            params![
                check.checked_at,
                check.status,
                check.breaking as i64,
                check.compatible as i64,
                target_id
            ],
        )?;

        transaction.commit()?;
        Ok(check_id)
    }

    pub fn history(&self, target_id: i64, limit: usize) -> Result<Vec<Check>> {
        let mut statement = self.conn.prepare(
            "SELECT id, checked_at, status, http_status, breaking, compatible, message, snapshot_id
             FROM checks WHERE target_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![target_id, limit as i64], |row| {
            Ok(Check {
                id: row.get(0)?,
                checked_at: row.get(1)?,
                status: row.get(2)?,
                http_status: row.get(3)?,
                breaking: row.get(4)?,
                compatible: row.get(5)?,
                message: row.get(6)?,
                snapshot_id: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn snapshot_count(&self, target_id: i64) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE target_id = ?1",
                [target_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
        CREATE TABLE targets (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE COLLATE NOCASE,
            url TEXT NOT NULL,
            headers TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            last_checked_at TEXT,
            last_status TEXT,
            last_breaking INTEGER NOT NULL DEFAULT 0,
            last_compatible INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE snapshots (
            id INTEGER PRIMARY KEY,
            target_id INTEGER NOT NULL REFERENCES targets(id) ON DELETE CASCADE,
            captured_at TEXT NOT NULL,
            format TEXT NOT NULL,
            etag TEXT,
            content TEXT NOT NULL,
            fact_count INTEGER NOT NULL
        );
        CREATE INDEX idx_snapshots_target ON snapshots(target_id, id DESC);

        CREATE TABLE checks (
            id INTEGER PRIMARY KEY,
            target_id INTEGER NOT NULL REFERENCES targets(id) ON DELETE CASCADE,
            checked_at TEXT NOT NULL,
            status TEXT NOT NULL,
            http_status INTEGER,
            breaking INTEGER NOT NULL DEFAULT 0,
            compatible INTEGER NOT NULL DEFAULT 0,
            message TEXT,
            snapshot_id INTEGER REFERENCES snapshots(id) ON DELETE SET NULL
        );
        CREATE INDEX idx_checks_target ON checks(target_id, id DESC);

        CREATE TABLE changes (
            id INTEGER PRIMARY KEY,
            check_id INTEGER NOT NULL REFERENCES checks(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            area TEXT NOT NULL,
            label TEXT NOT NULL,
            detail TEXT NOT NULL
        );
        CREATE INDEX idx_changes_check ON changes(check_id);
        PRAGMA user_version = 1;
        COMMIT;",
    )?;
    Ok(())
}

fn target_from_row(row: &Row<'_>) -> rusqlite::Result<Target> {
    let headers_json: String = row.get(3)?;
    let headers = serde_json::from_str(&headers_json).unwrap_or_default();
    Ok(Target {
        id: row.get(0)?,
        name: row.get(1)?,
        url: row.get(2)?,
        headers,
        created_at: row.get(4)?,
        last_checked_at: row.get(5)?,
        last_status: row.get(6)?,
        last_breaking: row.get(7)?,
        last_compatible: row.get(8)?,
    })
}

fn area_name(area: crate::spec::Area) -> &'static str {
    match area {
        crate::spec::Area::Operation => "operation",
        crate::spec::Area::Parameter => "parameter",
        crate::spec::Area::Request => "request",
        crate::spec::Area::Response => "response",
        crate::spec::Area::Value => "value",
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) {}

pub fn default_db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".apiwatch")
        .join("apiwatch.db")
}
