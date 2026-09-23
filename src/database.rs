use crate::youtube::{Snapshot, Video};
use anyhow::{Result, ensure};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::{path::Path, sync::Arc};

#[derive(Clone)]
pub struct Database(Arc<Mutex<Connection>>);
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    pub id: String,
    pub kind: String,
    pub youtube_id: String,
    pub url: String,
    pub title: String,
    pub description: String,
    pub folder: String,
    pub phase: String,
    pub error: Option<String>,
    pub feed_url: Option<String>,
    pub next_sync: i64,
    pub total: i64,
    pub uploaded: i64,
}
#[derive(Clone)]
pub struct Episode {
    pub video: Video,
    pub present: bool,
    pub state: String,
    pub bytes: u64,
    pub public_url: Option<String>,
    pub position: usize,
}
impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;
          CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL) STRICT;
          CREATE TABLE IF NOT EXISTS sources(id TEXT PRIMARY KEY,kind TEXT NOT NULL,youtube_id TEXT NOT NULL,url TEXT NOT NULL,title TEXT NOT NULL,description TEXT NOT NULL DEFAULT '',folder TEXT NOT NULL UNIQUE,phase TEXT NOT NULL DEFAULT 'idle',error TEXT,feed_url TEXT,next_sync INTEGER NOT NULL DEFAULT 0) STRICT;
          CREATE TABLE IF NOT EXISTS episodes(source TEXT NOT NULL REFERENCES sources(id),id TEXT NOT NULL,title TEXT NOT NULL,description TEXT NOT NULL DEFAULT '',published TEXT,duration REAL NOT NULL,available INTEGER NOT NULL,position INTEGER NOT NULL,present INTEGER NOT NULL DEFAULT 1,state TEXT NOT NULL,bytes INTEGER NOT NULL DEFAULT 0,public_url TEXT,PRIMARY KEY(source,id)) STRICT;
          CREATE INDEX IF NOT EXISTS episodes_presence ON episodes(source,present,state);
          PRAGMA user_version=1;
          UPDATE sources SET phase='idle' WHERE phase IS NOT 'idle' AND phase IS NOT 'error';")?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }
    pub fn sources(&self) -> Result<Vec<Source>> {
        let db = self.0.lock();
        let mut query = db.prepare("SELECT s.*,(SELECT count(*) FROM episodes WHERE source IS s.id AND present IS 1),(SELECT count(*) FROM episodes WHERE source IS s.id AND present IS 1 AND state IS 'uploaded') FROM sources s ORDER BY rowid DESC")?;
        Ok(query
            .query_map([], |row| {
                Ok(Source {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    youtube_id: row.get(2)?,
                    url: row.get(3)?,
                    title: row.get(4)?,
                    description: row.get(5)?,
                    folder: row.get(6)?,
                    phase: row.get(7)?,
                    error: row.get(8)?,
                    feed_url: row.get(9)?,
                    next_sync: row.get(10)?,
                    total: row.get(11)?,
                    uploaded: row.get(12)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn source(&self, id: &str) -> Result<Source> {
        self.sources()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow::anyhow!("Subscription not found"))
    }
    pub fn add(&self, kind: &str, id: &str, url: &str) -> Result<String> {
        let key = format!("{kind}:{id}");
        self.0.lock().execute("INSERT INTO sources(id,kind,youtube_id,url,title,folder) VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING", params![key,kind,id,url,"New subscription",format!("{kind}-{id}")])?;
        Ok(key)
    }
    pub fn snapshot(&self, id: &str, snapshot: &Snapshot) -> Result<()> {
        let mut db = self.0.lock();
        let tx = db.transaction()?;
        tx.execute("UPDATE episodes SET present=0 WHERE source IS ?", [id])?;
        for (position, v) in snapshot.videos.iter().enumerate() {
            tx.execute("INSERT INTO episodes(source,id,title,description,published,duration,available,position,state) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(source,id) DO UPDATE SET present=1,title=excluded.title,available=excluded.available,position=excluded.position,state=CASE WHEN episodes.state IS 'uploaded' THEN 'uploaded' WHEN excluded.available IS 0 THEN 'skipped' ELSE 'pending' END", params![id,v.id,v.title,v.description,v.published,v.duration,v.available,position as i64,if v.available {"pending"} else {"skipped"}])?;
        }
        tx.execute(
            "UPDATE sources SET title=?,description=? WHERE id IS ?",
            params![snapshot.title, snapshot.description, id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn episodes(&self, source: &str) -> Result<Vec<Episode>> {
        let db = self.0.lock();
        let mut query = db.prepare("SELECT id,title,description,published,duration,available,position,present,state,bytes,public_url FROM episodes WHERE source IS ? ORDER BY position,id")?;
        Ok(query
            .query_map([source], |row| {
                Ok(Episode {
                    video: Video {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        published: row.get(3)?,
                        duration: row.get(4)?,
                        available: row.get(5)?,
                    },
                    position: row.get::<_, i64>(6)? as usize,
                    present: row.get(7)?,
                    state: row.get(8)?,
                    bytes: row.get::<_, i64>(9)? as u64,
                    public_url: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn uploaded(&self, source: &str, video: &Video, bytes: u64, url: &str) -> Result<()> {
        self.0.lock().execute("UPDATE episodes SET state='uploaded',title=?,description=?,published=coalesce(?,published),duration=?,bytes=?,public_url=? WHERE source IS ? AND id IS ?", params![video.title,video.description,video.published,video.duration,bytes as i64,url,source,video.id])?;
        Ok(())
    }
    pub fn forget(&self, source: &str, video: &str) -> Result<()> {
        self.0.lock().execute(
            "DELETE FROM episodes WHERE source IS ? AND id IS ? AND present IS 0",
            params![source, video],
        )?;
        Ok(())
    }
    pub fn phase(&self, id: &str, phase: &str, error: Option<&str>) -> Result<()> {
        self.0.lock().execute(
            "UPDATE sources SET phase=?,error=? WHERE id IS ?",
            params![phase, error, id],
        )?;
        Ok(())
    }
    pub fn published(&self, id: &str, url: &str) -> Result<()> {
        self.0.lock().execute(
            "UPDATE sources SET feed_url=?,next_sync=? WHERE id IS ?",
            params![url, chrono::Utc::now().timestamp() + 3600, id],
        )?;
        Ok(())
    }
    pub fn postpone(&self, id: &str) -> Result<()> {
        self.0.lock().execute(
            "UPDATE sources SET next_sync=? WHERE id IS ?",
            params![chrono::Utc::now().timestamp() + 300, id],
        )?;
        Ok(())
    }
    pub fn bind_storage(&self, identity: &str) -> Result<()> {
        let db = self.0.lock();
        let existing: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key IS 'storage'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let count: i64 = db.query_row("SELECT count(*) FROM sources", [], |row| row.get(0))?;
        ensure!(
            count == 0 || existing.as_deref() == Some(identity),
            "Existing feeds are tied to their storage location; migration is required to move them"
        );
        db.execute("INSERT INTO settings(key,value) VALUES('storage',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [identity])?;
        Ok(())
    }
}
