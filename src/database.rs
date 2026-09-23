use crate::youtube::{Snapshot, Video};
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

mod migrations;

#[derive(Clone)]
pub struct Database(Arc<Connections>);

struct Connections {
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    path: PathBuf,
}

fn configure(connection: &Connection) -> Result<()> {
    // NORMAL + WAL keeps checkpoints consistent; a power loss can replay recent work.
    connection.execute_batch(
        "PRAGMA synchronous=NORMAL;
        PRAGMA foreign_keys=ON;
        PRAGMA busy_timeout=10000;
        PRAGMA cache_size=-2000;
        PRAGMA wal_autocheckpoint=1000;
        PRAGMA temp_store=MEMORY;
        PRAGMA mmap_size=0;",
    )?;
    Ok(())
}
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
impl Source {
    pub fn deletion_pending(&self) -> bool {
        matches!(self.phase.as_str(), "deleting" | "delete_error")
    }
}
impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        // Give in-memory test databases a unique name shared by their connections.
        let path = if path == Path::new(":memory:") {
            PathBuf::from(format!(
                "file:mazit-{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4()
            ))
        } else {
            std::path::absolute(path)?
        };
        let mut conn = Connection::open(&path)?;
        configure(&conn)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        migrations::run(&mut conn)?;
        conn.execute(
            "UPDATE sources SET phase='idle',next_sync=0 WHERE phase NOT IN ('idle','error','deleting','delete_error')",
            [],
        )?;
        Ok(Self(Arc::new(Connections {
            writer: Mutex::new(conn),
            readers: Mutex::new(Vec::new()),
            path,
        })))
    }

    fn read<T>(&self, query: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let cached = self.0.readers.lock().pop();
        let connection = match cached {
            Some(connection) => connection,
            None => {
                let connection = Connection::open(&self.0.path)?;
                configure(&connection)?;
                connection.execute_batch("PRAGMA query_only=ON;")?;
                connection
            }
        };
        // The pool lock is released during queries, independently of the writer mutex.
        let result = query(&connection);
        let mut readers = self.0.readers.lock();
        if readers.len() < 8 {
            readers.push(connection);
        }
        result
    }

    pub fn sources(&self) -> Result<Vec<Source>> {
        self.read(|db| {
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
        })
    }
    pub fn source(&self, id: &str) -> Result<Source> {
        self.sources()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow::anyhow!("Subscription not found"))
    }
    pub fn add(&self, kind: &str, id: &str, url: &str) -> Result<String> {
        let key = format!("{kind}:{id}");
        self.0.writer.lock().execute("INSERT INTO sources(id,kind,youtube_id,url,title,folder) VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING", params![key,kind,id,url,"New subscription",format!("{kind}-{id}")])?;
        Ok(key)
    }
    pub fn snapshot(&self, id: &str, snapshot: &Snapshot) -> Result<()> {
        let mut db = self.0.writer.lock();
        let tx = db.transaction()?;
        tx.execute("UPDATE episodes SET present=0 WHERE source IS ?", [id])?;
        for (position, v) in snapshot.videos.iter().enumerate() {
            tx.execute("INSERT INTO episodes(source,id,title,description,published,duration,available,position,state) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(source,id) DO UPDATE SET present=1,title=excluded.title,published=coalesce(episodes.published,excluded.published),available=excluded.available,position=excluded.position,state=CASE WHEN episodes.state IS 'uploaded' THEN 'uploaded' WHEN excluded.available IS 0 THEN 'skipped' ELSE 'pending' END", params![id,v.id,v.title,v.description,v.published,v.duration,v.available,position as i64,if v.available {"pending"} else {"skipped"}])?;
        }
        tx.execute(
            "UPDATE sources SET title=?,description=? WHERE id IS ?",
            params![snapshot.title, snapshot.description, id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn episodes(&self, source: &str) -> Result<Vec<Episode>> {
        self.read(|db| {
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
        })
    }
    pub fn uploaded(&self, source: &str, video: &Video, bytes: u64, url: &str) -> Result<()> {
        self.0.writer.lock().execute("UPDATE episodes SET state='uploaded',title=?,description=?,published=coalesce(?,published),duration=?,bytes=?,public_url=? WHERE source IS ? AND id IS ?", params![video.title,video.description,video.published,video.duration,bytes as i64,url,source,video.id])?;
        Ok(())
    }
    pub fn pending_episodes(&self, source: &str) -> Result<Vec<Episode>> {
        Ok(self
            .episodes(source)?
            .into_iter()
            .filter(|episode| {
                episode.present && episode.video.available && episode.state != "uploaded"
            })
            .collect())
    }
    pub fn skip_until_next_sync(&self, source: &str, video: &str) -> Result<()> {
        self.0.writer.lock().execute(
            "UPDATE episodes SET available=0,state='skipped' WHERE source IS ? AND id IS ? AND state IS NOT 'uploaded'",
            params![source, video],
        )?;
        Ok(())
    }
    pub fn forget(&self, source: &str, video: &str) -> Result<()> {
        self.0.writer.lock().execute(
            "DELETE FROM episodes WHERE source IS ? AND id IS ? AND present IS 0",
            params![source, video],
        )?;
        Ok(())
    }
    pub fn delete_source(&self, id: &str) -> Result<()> {
        let mut db = self.0.writer.lock();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM episodes WHERE source IS ?", [id])?;
        tx.execute("DELETE FROM sources WHERE id IS ?", [id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn phase(&self, id: &str, phase: &str, error: Option<&str>) -> Result<()> {
        self.0.writer.lock().execute(
            "UPDATE sources SET phase=?,error=? WHERE id IS ?",
            params![phase, error, id],
        )?;
        Ok(())
    }
    pub fn published(&self, id: &str, url: &str) -> Result<()> {
        self.0.writer.lock().execute(
            "UPDATE sources SET feed_url=?,next_sync=? WHERE id IS ?",
            params![url, chrono::Utc::now().timestamp() + 3600, id],
        )?;
        Ok(())
    }
    pub fn postpone(&self, id: &str) -> Result<()> {
        self.0.writer.lock().execute(
            "UPDATE sources SET next_sync=? WHERE id IS ?",
            params![chrono::Utc::now().timestamp() + 300, id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_episodes_become_pending_again_when_the_next_listing_is_available() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        let source = db
            .add("playlist", "test", "https://www.youtube.com")
            .unwrap();
        let mut listing = snapshot();
        listing.videos[0].available = false;
        db.snapshot(&source, &listing).unwrap();
        db.skip_until_next_sync(&source, &listing.videos[1].id)
            .unwrap();
        assert_eq!(db.pending_episodes(&source).unwrap().len(), 6);
        assert!(
            db.episodes(&source).unwrap()[..2]
                .iter()
                .all(|episode| episode.state == "skipped" && episode.present)
        );
        listing.videos[0].available = true;
        db.snapshot(&source, &listing).unwrap();
        assert_eq!(db.pending_episodes(&source).unwrap().len(), 8);
        assert!(
            db.episodes(&source)
                .unwrap()
                .iter()
                .all(|episode| episode.state == "pending")
        );
    }

    #[test]
    fn wal_readers_do_not_wait_for_the_writer_and_use_consistent_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(&directory.path().join("concurrent.sqlite")).unwrap();
        let id = db
            .add("playlist", "test", "https://www.youtube.com")
            .unwrap();
        let mut writer = db.0.writer.lock();
        let tx = writer.transaction().unwrap();
        tx.execute(
            "UPDATE sources SET title='Uncommitted' WHERE id IS ?",
            [&id],
        )
        .unwrap();
        // These queries must finish before the writer's transaction or mutex is released.
        std::thread::scope(|scope| {
            let readers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        assert_eq!(db.source(&id).unwrap().title, "New subscription");
                    })
                })
                .collect();
            for reader in readers {
                reader.join().unwrap();
            }
        });
        tx.commit().unwrap();
        assert_eq!(db.source(&id).unwrap().title, "Uncommitted");
        db.read(|reader| {
            for connection in [&*writer, reader] {
                for (pragma, expected) in [
                    ("synchronous", 1),
                    ("foreign_keys", 1),
                    ("busy_timeout", 10000),
                    ("cache_size", -2000),
                    ("wal_autocheckpoint", 1000),
                    ("temp_store", 2),
                    ("mmap_size", 0),
                ] {
                    let actual: i64 =
                        connection.pragma_query_value(None, pragma, |row| row.get(0))?;
                    assert_eq!(actual, expected, "{pragma}");
                }
                let mode: String =
                    connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
                assert_eq!(mode, "wal");
            }
            assert!(reader.execute("DELETE FROM sources", []).is_err());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn deletion_is_transactional_and_pending_deletions_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("delete.sqlite");
        let db = Database::open(&path).unwrap();
        let deleted = db
            .add("playlist", "deleted", "https://www.youtube.com")
            .unwrap();
        let retained = db
            .add("playlist", "retained", "https://www.youtube.com")
            .unwrap();
        for id in [&deleted, &retained] {
            db.snapshot(id, &snapshot()).unwrap();
        }
        db.phase(&deleted, "deleting", None).unwrap();
        db.phase(&retained, "delete_error", Some("Retry deletion"))
            .unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(db.source(&deleted).unwrap().phase, "deleting");
        assert_eq!(db.source(&retained).unwrap().phase, "delete_error");
        db.delete_source(&deleted).unwrap();
        db.delete_source(&deleted).unwrap();
        assert!(db.source(&deleted).is_err());
        assert!(db.episodes(&deleted).unwrap().is_empty());
        assert_eq!(db.episodes(&retained).unwrap().len(), 8);
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            title: "Eight videos".into(),
            description: String::new(),
            cover_url: None,
            videos: (0..8)
                .map(|index| Video {
                    id: format!("video{index}"),
                    title: format!("Video {index}"),
                    description: String::new(),
                    published: Some("2011-01-11".into()),
                    duration: 300.0,
                    available: true,
                })
                .collect(),
        }
    }

    #[test]
    fn flat_dates_backfill_without_losing_exact_dates_or_upload_receipts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dates.sqlite");
        let db = Database::open(&path).unwrap();
        let source = db
            .add("playlist", "test", "https://www.youtube.com")
            .unwrap();
        let mut snapshot = snapshot();
        snapshot.videos.truncate(3);
        snapshot.videos[0].published = None;
        snapshot.videos[1].published = Some("2011-01-11T13:41:25+00:00".into());
        snapshot.videos[2].published = Some("2011-09-24T00:00:00+00:00".into());
        db.snapshot(&source, &snapshot).unwrap();
        for video in &snapshot.videos {
            db.uploaded(
                &source,
                video,
                123,
                "https://audio.example.com/existing.m4a",
            )
            .unwrap();
        }
        // Repeated relative-date estimates can drift; keep the first saved value.
        for video in &mut snapshot.videos {
            video.published = Some("2011-09-25T00:00:00+00:00".into());
        }
        db.snapshot(&source, &snapshot).unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        for video in &mut snapshot.videos {
            video.published = Some("2011-09-26T00:00:00+00:00".into());
        }
        db.snapshot(&source, &snapshot).unwrap();
        let episodes = db.episodes(&source).unwrap();
        assert_eq!(
            episodes
                .iter()
                .map(|e| e.video.published.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("2011-09-25T00:00:00+00:00"),
                Some("2011-01-11T13:41:25+00:00"),
                Some("2011-09-24T00:00:00+00:00"),
            ]
        );
        assert!(db.pending_episodes(&source).unwrap().is_empty());
        for episode in episodes {
            assert_eq!(episode.state, "uploaded");
            assert_eq!(episode.bytes, 123);
            assert_eq!(
                episode.public_url.as_deref(),
                Some("https://audio.example.com/existing.m4a")
            );
        }
    }

    #[test]
    fn media_dates_can_improve_estimates_but_missing_media_dates_keep_them() {
        let db = Database::open(Path::new(":memory:")).unwrap();
        let source = db
            .add("playlist", "test", "https://www.youtube.com")
            .unwrap();
        let mut snapshot = snapshot();
        snapshot.videos.truncate(2);
        for video in &mut snapshot.videos {
            video.published = Some("2011-09-24T00:00:00+00:00".into());
        }
        db.snapshot(&source, &snapshot).unwrap();
        snapshot.videos[0].published = Some("2011-01-11T13:41:25+00:00".into());
        snapshot.videos[1].published = None;
        for video in &snapshot.videos {
            db.uploaded(
                &source,
                video,
                123,
                "https://audio.example.com/existing.m4a",
            )
            .unwrap();
        }
        let episodes = db.episodes(&source).unwrap();
        assert_eq!(
            episodes[0].video.published.as_deref(),
            Some("2011-01-11T13:41:25+00:00")
        );
        assert_eq!(
            episodes[1].video.published.as_deref(),
            Some("2011-09-24T00:00:00+00:00")
        );
    }

    #[test]
    fn crash_after_four_uploads_child() {
        let Some(path) = std::env::var_os("MAZIT_CHECKPOINT_TEST_DB") else {
            return;
        };
        let db = Database::open(Path::new(&path)).unwrap();
        let source = db
            .add(
                "playlist",
                "test",
                "https://www.youtube.com/playlist?list=test",
            )
            .unwrap();
        let snapshot = snapshot();
        db.snapshot(&source, &snapshot).unwrap();
        db.published(&source, "https://audio.example.com/rss.xml")
            .unwrap();
        db.phase(&source, "downloading", None).unwrap();
        for video in &snapshot.videos[..4] {
            db.uploaded(
                &source,
                video,
                123,
                &format!("https://audio.example.com/{}.m4a", video.id),
            )
            .unwrap();
        }
        // Exit without dropping the connection, checkpointing WAL on close, or cleaning up.
        std::process::exit(86);
    }

    #[test]
    fn crash_after_four_of_eight_uploads_resumes_only_the_remaining_four() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mazit.sqlite");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "database::tests::crash_after_four_uploads_child",
                "--nocapture",
            ])
            .env("MAZIT_CHECKPOINT_TEST_DB", &path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let db = Database::open(&path).unwrap();
        let source = db.source("playlist:test").unwrap();
        assert_eq!((source.uploaded, source.total), (4, 8));
        assert_eq!(source.phase, "idle");
        assert_eq!(source.next_sync, 0);
        assert_eq!(
            source.feed_url.as_deref(),
            Some("https://audio.example.com/rss.xml")
        );
        db.snapshot(&source.id, &snapshot()).unwrap();
        let pending = db.pending_episodes(&source.id).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|episode| episode.video.id.as_str())
                .collect::<Vec<_>>(),
            ["video4", "video5", "video6", "video7"]
        );
        for episode in db.episodes(&source.id).unwrap().iter().take(4) {
            assert_eq!(episode.state, "uploaded");
            assert_eq!(episode.bytes, 123);
            assert_eq!(
                episode.public_url.as_deref(),
                Some(format!("https://audio.example.com/{}.m4a", episode.video.id).as_str())
            );
        }
        for episode in pending {
            db.uploaded(
                &source.id,
                &episode.video,
                123,
                &format!("https://audio.example.com/{}.m4a", episode.video.id),
            )
            .unwrap();
        }
        assert!(db.pending_episodes(&source.id).unwrap().is_empty());
        assert_eq!(db.source(&source.id).unwrap().uploaded, 8);
        let feed = crate::rss::render(
            &source,
            &db.episodes(&source.id).unwrap(),
            "https://audio.example.com/rss.xml",
            None,
        )
        .unwrap();
        assert_eq!(feed.matches("<item>").count(), 8);
        let connection = db.0.writer.lock();
        let has_settings: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IS 'settings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_settings);
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn interrupted_publication_and_cleanup_are_retried_on_restart() {
        for phase in ["scanning", "publishing", "cleaning"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("mazit.sqlite");
            let db = Database::open(&path).unwrap();
            let source = db
                .add("playlist", "test", "https://www.youtube.com")
                .unwrap();
            db.published(&source, "https://audio.example.com/rss.xml")
                .unwrap();
            db.phase(&source, phase, None).unwrap();
            drop(db);
            let reopened = Database::open(&path).unwrap();
            assert_eq!(reopened.source(&source).unwrap().next_sync, 0);
        }
    }
}
