use std::path::Path;

use manga_core::ComicSummary;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub source: String,
    pub comic: ComicSummary,
    pub chapter_id: String,
    pub chapter_title: String,
    pub page: usize,
    pub updated_at: i64,
}

/// Reading progress, one row per (source, comic).
pub struct History {
    conn: Mutex<Connection>,
}

impl History {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS history (
                source TEXT NOT NULL,
                comic_id TEXT NOT NULL,
                comic_json TEXT NOT NULL,
                chapter_id TEXT NOT NULL,
                chapter_title TEXT NOT NULL,
                page INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (source, comic_id)
             );
             CREATE INDEX IF NOT EXISTS history_updated ON history(updated_at DESC);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn record(&self, e: &HistoryEntry) -> rusqlite::Result<()> {
        let json = serde_json::to_string(&e.comic).unwrap_or_default();
        self.conn.lock().execute(
            "INSERT INTO history (source, comic_id, comic_json, chapter_id, chapter_title, page, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(source, comic_id) DO UPDATE SET
               comic_json=excluded.comic_json, chapter_id=excluded.chapter_id,
               chapter_title=excluded.chapter_title, page=excluded.page, updated_at=excluded.updated_at",
            params![e.source, e.comic.id, json, e.chapter_id, e.chapter_title, e.page as i64, e.updated_at],
        )?;
        Ok(())
    }

    pub fn get(&self, source: &str, comic_id: &str) -> Option<HistoryEntry> {
        self.conn
            .lock()
            .query_row(
                "SELECT source, comic_json, chapter_id, chapter_title, page, updated_at
                 FROM history WHERE source=?1 AND comic_id=?2",
                params![source, comic_id],
                row_to_entry,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn recent(&self, limit: usize) -> Vec<HistoryEntry> {
        let conn = self.conn.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT source, comic_json, chapter_id, chapter_title, page, updated_at
             FROM history ORDER BY updated_at DESC, rowid DESC LIMIT ?1",
        ) else {
            return vec![];
        };
        stmt.query_map(params![limit as i64], row_to_entry).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    pub fn remove(&self, source: &str, comic_id: &str) {
        let _ =
            self.conn.lock().execute("DELETE FROM history WHERE source=?1 AND comic_id=?2", params![source, comic_id]);
    }
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let json: String = r.get(1)?;
    let comic = serde_json::from_str(&json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
    Ok(HistoryEntry {
        source: r.get(0)?,
        comic,
        chapter_id: r.get(2)?,
        chapter_title: r.get(3)?,
        page: r.get::<_, i64>(4)? as usize,
        updated_at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, page: usize, t: i64) -> HistoryEntry {
        HistoryEntry {
            source: "local".into(),
            comic: ComicSummary { id: id.into(), title: id.into(), subtitle: None, cover: None, tags: vec![] },
            chapter_id: "c1".into(),
            chapter_title: "第1话".into(),
            page,
            updated_at: t,
        }
    }

    #[test]
    fn upsert_and_order() {
        let h = History::in_memory().unwrap();
        h.record(&entry("a", 1, 10)).unwrap();
        h.record(&entry("b", 2, 20)).unwrap();
        h.record(&entry("a", 5, 30)).unwrap();
        let r = h.recent(10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].comic.id, "a");
        assert_eq!(r[0].page, 5);
        assert_eq!(h.get("local", "b").unwrap().page, 2);
        h.remove("local", "a");
        assert!(h.get("local", "a").is_none());
    }
}
