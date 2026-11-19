use std::{fs, io, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ACTIVE: &str = "state IN ('receiving', 'processing', 'ready', 'delivering')";
const COLUMNS: &str = "id, state, next_index, pending, sizes, error, directory";

#[derive(Debug, Serialize)]
pub struct Job {
    pub id: String,
    pub state: String,
    pub next_index: usize,
    pub pending: bool,
    pub sizes: Vec<u64>,
    pub error: Option<String>,
    #[serde(skip)]
    pub directory: Option<String>,
}

fn job(row: &Row<'_>) -> rusqlite::Result<Job> {
    let sizes: String = row.get(4)?;
    Ok(Job {
        id: row.get(0)?,
        state: row.get(1)?,
        next_index: usize::from(row.get::<_, u8>(2)?),
        pending: row.get(3)?,
        sizes: serde_json::from_str(&sizes).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, error.into())
        })?,
        error: row.get(5)?,
        directory: row.get(6)?,
    })
}

#[derive(Debug)]
pub enum Reservation {
    New(String),
    Duplicate(Job),
    Busy,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAction {
    Begin,
    Ack,
}

pub struct Store(Connection);

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS jobs (
                 id TEXT PRIMARY KEY,
                 message_hash TEXT NOT NULL UNIQUE,
                 state TEXT NOT NULL,
                 next_index INTEGER NOT NULL DEFAULT 0,
                 pending INTEGER NOT NULL DEFAULT 0,
                 sizes TEXT NOT NULL DEFAULT '[]',
                 error TEXT,
                 directory TEXT,
                 record TEXT,
                 expires INTEGER
             );
             CREATE UNIQUE INDEX IF NOT EXISTS one_active ON jobs((1)) WHERE {ACTIVE};"
        ))?;
        Ok(Self(db))
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<Job>> {
        self.0
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
                [id],
                job,
            )
            .optional()
    }

    pub fn by_message(&self, hash: &str) -> rusqlite::Result<Option<Job>> {
        self.0
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE message_hash = ?1"),
                [hash],
                job,
            )
            .optional()
    }

    pub fn active(&self) -> rusqlite::Result<Option<Job>> {
        self.0
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE {ACTIVE}"),
                [],
                job,
            )
            .optional()
    }

    pub fn latest(&self) -> rusqlite::Result<Option<Job>> {
        self.0
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs ORDER BY rowid DESC LIMIT 1"),
                [],
                job,
            )
            .optional()
    }

    pub fn reserve(&self, hash: &str) -> rusqlite::Result<Reservation> {
        if let Some(existing) = self.by_message(hash)? {
            return Ok(Reservation::Duplicate(existing));
        }
        if self.active()?.is_some() {
            return Ok(Reservation::Busy);
        }
        let id = Uuid::new_v4().to_string();
        self.0.execute(
            "INSERT INTO jobs (id, message_hash, state) VALUES (?1, ?2, 'receiving')",
            params![id, hash],
        )?;
        Ok(Reservation::New(id))
    }

    pub fn processing(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self.0.execute(
            "UPDATE jobs SET state = 'processing' WHERE id = ?1 AND state = 'receiving'",
            [id],
        )? == 1)
    }

    pub fn abandon_upload(&self, id: &str) -> rusqlite::Result<()> {
        self.0.execute(
            "DELETE FROM jobs WHERE id = ?1 AND state = 'receiving'",
            [id],
        )?;
        Ok(())
    }

    pub fn ready(
        &self,
        id: &str,
        directory: &str,
        sizes: &str,
        record: &str,
    ) -> rusqlite::Result<bool> {
        Ok(self.0.execute(
            "UPDATE jobs SET state = 'ready', directory = ?2, sizes = ?3, record = ?4,
             expires = unixepoch() + 1200 WHERE id = ?1 AND state = 'processing'",
            params![id, directory, sizes, record],
        )? == 1)
    }

    pub fn fail(&self, id: &str, error: &str) -> rusqlite::Result<bool> {
        Ok(self.0.execute(
            &format!(
                "UPDATE jobs SET state = 'failed', error = ?2, pending = 0, directory = NULL,
                 sizes = '[]', record = NULL, expires = NULL WHERE id = ?1 AND {ACTIVE}"
            ),
            params![id, error],
        )? == 1)
    }

    pub fn expire(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self.0.execute(
            "UPDATE jobs SET state = 'failed',
             error = CASE WHEN pending THEN 'delivery_uncertain' ELSE 'expired' END,
             pending = 0, directory = NULL, sizes = '[]', record = NULL, expires = NULL
             WHERE id = ?1 AND expires <= unixepoch()",
            [id],
        )? == 1)
    }

    pub fn delivery(&self, id: &str, index: u8, action: DeliveryAction) -> rusqlite::Result<bool> {
        match action {
            DeliveryAction::Begin => Ok(self.0.execute(
                "UPDATE jobs SET state = 'delivering', pending = 1
                 WHERE id = ?1 AND next_index = ?2 AND NOT pending
                 AND state IN ('ready', 'delivering') AND expires > unixepoch()",
                params![id, index],
            )? == 1),
            DeliveryAction::Ack => {
                self.0.execute(
                    "UPDATE jobs SET next_index = next_index + 1, pending = 0,
                     state = iif(next_index + 1 = json_array_length(sizes), 'delivered', state),
                     directory = iif(state_after.is_last, NULL, directory),
                     record = iif(state_after.is_last, NULL, record),
                     sizes = iif(state_after.is_last, '[]', sizes),
                     expires = iif(state_after.is_last, NULL, expires)
                     FROM (SELECT next_index + 1 = json_array_length(sizes) AS is_last
                           FROM jobs WHERE id = ?1) AS state_after
                     WHERE id = ?1 AND next_index = ?2 AND pending AND state = 'delivering'
                     AND expires > unixepoch()",
                    params![id, index],
                )?;
                Ok(self
                    .get(id)?
                    .is_some_and(|job| job.next_index > usize::from(index)))
            }
        }
    }

    pub fn recover(&self) -> rusqlite::Result<()> {
        self.0
            .execute("DELETE FROM jobs WHERE state = 'receiving'", [])?;
        if let Some(job) = self.active()? {
            if job.pending {
                self.fail(&job.id, "delivery_uncertain")?;
            } else if job.state == "processing" {
                self.fail(&job.id, "processing_interrupted")?;
            }
        }
        Ok(())
    }
}

impl Job {
    pub fn artifact(&self, media: &Path, index: usize) -> Option<std::path::PathBuf> {
        let directory = self.directory.as_deref()?;
        if Uuid::parse_str(&self.id).is_err()
            || !directory.starts_with("vidora-")
            || !directory
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || index >= self.sizes.len()
        {
            return None;
        }
        Some(
            media
                .join(&self.id)
                .join(directory)
                .join(format!("clip-{index:03}.mp4")),
        )
    }

    pub fn intact(&self, media: &Path) -> bool {
        !self.sizes.is_empty()
            && self.sizes.iter().enumerate().all(|(index, size)| {
                self.artifact(media, index)
                    .and_then(|path| fs::symlink_metadata(path).ok())
                    .is_some_and(|metadata| metadata.is_file() && metadata.len() == *size)
            })
    }
}

pub fn remove_media(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
