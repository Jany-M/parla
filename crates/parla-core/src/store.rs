use crate::types::*;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA_VERSION: i64 = 1;

/// SQLite-backed storage. Cheap to clone; all access is serialised by one mutex.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // WAL: a crash mid-meeting never loses finished segments.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v > SCHEMA_VERSION {
            bail!("database was created by a newer version of the app (schema {v})");
        }
        if v < 1 {
            conn.execute_batch(
                "CREATE TABLE meetings(
                    id INTEGER PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT '',
                    started_at INTEGER NOT NULL,
                    ended_at INTEGER,
                    source TEXT NOT NULL,
                    notes TEXT NOT NULL DEFAULT '',
                    summary TEXT NOT NULL DEFAULT ''
                 );
                 CREATE TABLE speakers(
                    id INTEGER PRIMARY KEY,
                    meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                    label TEXT NOT NULL,
                    name TEXT,
                    is_me INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE segments(
                    id INTEGER PRIMARY KEY,
                    meeting_id INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                    stream TEXT NOT NULL,
                    speaker_id INTEGER REFERENCES speakers(id) ON DELETE SET NULL,
                    t0 REAL NOT NULL,
                    t1 REAL NOT NULL,
                    lang TEXT,
                    text TEXT NOT NULL,
                    translation TEXT,
                    translation_lang TEXT
                 );
                 CREATE INDEX idx_segments_meeting ON segments(meeting_id, t0);
                 CREATE INDEX idx_speakers_meeting ON speakers(meeting_id);",
            )?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- meetings ----------------------------------------------------------

    pub fn create_meeting(&self, source: &str) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO meetings(title, started_at, source) VALUES('', ?1, ?2)",
            params![now_secs(), source],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn end_meeting(&self, id: i64) -> Result<()> {
        self.db().execute("UPDATE meetings SET ended_at=?1 WHERE id=?2 AND ended_at IS NULL", params![now_secs(), id])?;
        Ok(())
    }

    pub fn set_title(&self, id: i64, title: &str) -> Result<()> {
        self.db().execute("UPDATE meetings SET title=?1 WHERE id=?2", params![title.trim(), id])?;
        Ok(())
    }

    pub fn set_notes(&self, id: i64, notes: &str) -> Result<()> {
        self.db().execute("UPDATE meetings SET notes=?1 WHERE id=?2", params![notes, id])?;
        Ok(())
    }

    pub fn set_summary(&self, id: i64, summary: &str) -> Result<()> {
        self.db().execute("UPDATE meetings SET summary=?1 WHERE id=?2", params![summary, id])?;
        Ok(())
    }

    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        self.db().execute("DELETE FROM meetings WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn delete_all_meetings(&self) -> Result<()> {
        self.db().execute("DELETE FROM meetings", [])?;
        Ok(())
    }

    /// Meetings that were never closed (the app crashed or was killed mid-recording).
    pub fn close_dangling_meetings(&self) -> Result<usize> {
        let db = self.db();
        // End them at the time of their last segment, or at start if empty.
        let n = db.execute(
            "UPDATE meetings SET ended_at = started_at + COALESCE(
                (SELECT CAST(MAX(t1) AS INTEGER) FROM segments WHERE meeting_id = meetings.id), 0)
             WHERE ended_at IS NULL",
            [],
        )?;
        Ok(n)
    }

    pub fn list_meetings(&self) -> Result<Vec<MeetingListItem>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT m.id, m.title, m.started_at, m.ended_at, m.source,
                    (SELECT COUNT(*) FROM segments s WHERE s.meeting_id = m.id)
             FROM meetings m ORDER BY m.started_at DESC, m.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(MeetingListItem {
                id: r.get(0)?,
                title: r.get(1)?,
                started_at: r.get(2)?,
                ended_at: r.get(3)?,
                source: r.get(4)?,
                segment_count: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get_meeting(&self, id: i64) -> Result<Option<MeetingDetail>> {
        let db = self.db();
        let meeting = db
            .query_row(
                "SELECT id, title, started_at, ended_at, source, notes, summary FROM meetings WHERE id=?1",
                params![id],
                |r| {
                    Ok(MeetingInfo {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                        source: r.get(4)?,
                        notes: r.get(5)?,
                        summary: r.get(6)?,
                    })
                },
            )
            .optional()?;
        let Some(meeting) = meeting else { return Ok(None) };
        drop(db);
        Ok(Some(MeetingDetail { meeting, speakers: self.speakers(id)?, segments: self.segments(id)? }))
    }

    // ---- speakers ----------------------------------------------------------

    pub fn speakers(&self, meeting_id: i64) -> Result<Vec<Speaker>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id, meeting_id, label, name, is_me FROM speakers WHERE meeting_id=?1 ORDER BY id")?;
        let rows = st.query_map(params![meeting_id], row_speaker)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Creates "Me" or the next free "Speaker N" (N survives merges: it is max+1, not count+1).
    pub fn create_speaker(&self, meeting_id: i64, is_me: bool) -> Result<Speaker> {
        let existing = self.speakers(meeting_id)?;
        let label = if is_me {
            "Me".to_string()
        } else {
            let max = existing
                .iter()
                .filter_map(|s| s.label.strip_prefix("Speaker ").and_then(|n| n.parse::<u32>().ok()))
                .max()
                .unwrap_or(0);
            format!("Speaker {}", max + 1)
        };
        let db = self.db();
        db.execute(
            "INSERT INTO speakers(meeting_id, label, is_me) VALUES(?1, ?2, ?3)",
            params![meeting_id, label, is_me as i64],
        )?;
        Ok(Speaker { id: db.last_insert_rowid(), meeting_id, label, name: None, is_me })
    }

    pub fn rename_speaker(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        let v: Option<&str> = if name.is_empty() { None } else { Some(name) };
        self.db().execute("UPDATE speakers SET name=?1 WHERE id=?2", params![v, id])?;
        Ok(())
    }

    /// Moves all segments of `from` onto `into` and removes `from`.
    pub fn merge_speakers(&self, from: i64, into: i64) -> Result<()> {
        if from == into {
            bail!("cannot merge a speaker into itself");
        }
        let mut db = self.db();
        let tx = db.transaction()?;
        let get = |id: i64| -> Result<(i64, bool)> {
            tx.query_row("SELECT meeting_id, is_me FROM speakers WHERE id=?1", params![id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0))
            })
            .optional()?
            .context("unknown speaker")
        };
        let (m_from, me_from) = get(from)?;
        let (m_into, _) = get(into)?;
        if m_from != m_into {
            bail!("speakers belong to different meetings");
        }
        if me_from {
            bail!("the 'Me' speaker cannot be merged away");
        }
        tx.execute("UPDATE segments SET speaker_id=?1 WHERE speaker_id=?2", params![into, from])?;
        tx.execute("DELETE FROM speakers WHERE id=?1", params![from])?;
        tx.commit()?;
        Ok(())
    }

    // ---- segments ----------------------------------------------------------

    pub fn insert_segment(&self, s: &NewSegment) -> Result<Segment> {
        let db = self.db();
        db.execute(
            "INSERT INTO segments(meeting_id, stream, speaker_id, t0, t1, lang, text) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![s.meeting_id, s.stream.as_str(), s.speaker_id, s.t0, s.t1, s.lang, s.text],
        )?;
        Ok(Segment {
            id: db.last_insert_rowid(),
            meeting_id: s.meeting_id,
            stream: s.stream,
            speaker_id: s.speaker_id,
            t0: s.t0,
            t1: s.t1,
            lang: s.lang.clone(),
            text: s.text.clone(),
            translation: None,
            translation_lang: None,
        })
    }

    pub fn set_translation(&self, segment_id: i64, text: &str, lang: &str) -> Result<()> {
        self.db().execute(
            "UPDATE segments SET translation=?1, translation_lang=?2 WHERE id=?3",
            params![text, lang, segment_id],
        )?;
        Ok(())
    }

    pub fn segments(&self, meeting_id: i64) -> Result<Vec<Segment>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, meeting_id, stream, speaker_id, t0, t1, lang, text, translation, translation_lang
             FROM segments WHERE meeting_id=?1 ORDER BY t0, id",
        )?;
        let rows = st.query_map(params![meeting_id], |r| {
            let stream: String = r.get(2)?;
            Ok(Segment {
                id: r.get(0)?,
                meeting_id: r.get(1)?,
                stream: StreamKind::parse(&stream).unwrap_or(StreamKind::System),
                speaker_id: r.get(3)?,
                t0: r.get(4)?,
                t1: r.get(5)?,
                lang: r.get(6)?,
                text: r.get(7)?,
                translation: r.get(8)?,
                translation_lang: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn row_speaker(r: &rusqlite::Row) -> rusqlite::Result<Speaker> {
    Ok(Speaker {
        id: r.get(0)?,
        meeting_id: r.get(1)?,
        label: r.get(2)?,
        name: r.get(3)?,
        is_me: r.get::<_, i64>(4)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(meeting_id: i64, speaker_id: Option<i64>, t0: f64, text: &str) -> NewSegment {
        NewSegment { meeting_id, stream: StreamKind::System, speaker_id, t0, t1: t0 + 1.0, lang: Some("en".into()), text: text.into() }
    }

    #[test]
    fn meeting_lifecycle_and_listing() {
        let s = Store::open_in_memory().unwrap();
        let a = s.create_meeting("live").unwrap();
        let b = s.create_meeting("file").unwrap();
        s.insert_segment(&seg(a, None, 0.0, "hello")).unwrap();
        s.set_title(a, "  Weekly sync ").unwrap();
        s.end_meeting(a).unwrap();
        let list = s.list_meetings().unwrap();
        assert_eq!(list.len(), 2);
        let la = list.iter().find(|m| m.id == a).unwrap();
        assert_eq!((la.title.as_str(), la.segment_count), ("Weekly sync", 1));
        assert!(la.ended_at.is_some());
        s.delete_meeting(b).unwrap();
        assert_eq!(s.list_meetings().unwrap().len(), 1);
    }

    #[test]
    fn segments_are_ordered_by_time_not_insert_order() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        s.insert_segment(&seg(m, None, 5.0, "second")).unwrap();
        s.insert_segment(&seg(m, None, 1.0, "first")).unwrap();
        let t: Vec<_> = s.segments(m).unwrap().into_iter().map(|x| x.text).collect();
        assert_eq!(t, ["first", "second"]);
    }

    #[test]
    fn speaker_labels_never_collide_after_merge() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        let me = s.create_speaker(m, true).unwrap();
        let a = s.create_speaker(m, false).unwrap();
        let b = s.create_speaker(m, false).unwrap();
        assert_eq!((me.label.as_str(), a.label.as_str(), b.label.as_str()), ("Me", "Speaker 1", "Speaker 2"));
        s.merge_speakers(a.id, b.id).unwrap(); // leaves only "Speaker 2"
        let c = s.create_speaker(m, false).unwrap();
        assert_eq!(c.label, "Speaker 3");
    }

    #[test]
    fn merge_moves_segments_and_guards() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        let me = s.create_speaker(m, true).unwrap();
        let a = s.create_speaker(m, false).unwrap();
        let b = s.create_speaker(m, false).unwrap();
        s.insert_segment(&seg(m, Some(a.id), 0.0, "x")).unwrap();
        s.insert_segment(&seg(m, Some(b.id), 2.0, "y")).unwrap();
        s.merge_speakers(a.id, b.id).unwrap();
        let segs = s.segments(m).unwrap();
        assert!(segs.iter().all(|x| x.speaker_id == Some(b.id)));
        assert_eq!(s.speakers(m).unwrap().len(), 2);
        assert!(s.merge_speakers(b.id, b.id).is_err());
        assert!(s.merge_speakers(me.id, b.id).is_err(), "Me must not be merged away");
        let other = s.create_meeting("live").unwrap();
        let o = s.create_speaker(other, false).unwrap();
        assert!(s.merge_speakers(b.id, o.id).is_err(), "cross-meeting merge");
    }

    #[test]
    fn rename_and_clear_name() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        let a = s.create_speaker(m, false).unwrap();
        s.rename_speaker(a.id, "  Anna ").unwrap();
        assert_eq!(s.speakers(m).unwrap()[0].display(), "Anna");
        s.rename_speaker(a.id, "   ").unwrap();
        assert_eq!(s.speakers(m).unwrap()[0].display(), "Speaker 1");
    }

    #[test]
    fn translation_roundtrip_and_detail() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        let seg = s.insert_segment(&seg(m, None, 0.0, "hello")).unwrap();
        s.set_translation(seg.id, "ciao", "it").unwrap();
        s.set_notes(m, "note").unwrap();
        s.set_summary(m, "sum").unwrap();
        let d = s.get_meeting(m).unwrap().unwrap();
        assert_eq!(d.segments[0].translation.as_deref(), Some("ciao"));
        assert_eq!((d.meeting.notes.as_str(), d.meeting.summary.as_str()), ("note", "sum"));
        assert!(s.get_meeting(999).unwrap().is_none());
    }

    #[test]
    fn deleting_a_meeting_cascades() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        let sp = s.create_speaker(m, false).unwrap();
        s.insert_segment(&seg(m, Some(sp.id), 0.0, "x")).unwrap();
        s.delete_meeting(m).unwrap();
        assert!(s.segments(m).unwrap().is_empty());
        assert!(s.speakers(m).unwrap().is_empty());
    }

    #[test]
    fn dangling_meetings_are_closed_at_last_segment() {
        let s = Store::open_in_memory().unwrap();
        let m = s.create_meeting("live").unwrap();
        s.insert_segment(&NewSegment { t0: 90.0, t1: 95.0, ..seg(m, None, 0.0, "x") }).unwrap();
        assert_eq!(s.close_dangling_meetings().unwrap(), 1);
        let d = s.get_meeting(m).unwrap().unwrap().meeting;
        assert_eq!(d.ended_at, Some(d.started_at + 95));
        assert_eq!(s.close_dangling_meetings().unwrap(), 0);
    }

    #[test]
    fn file_database_persists_and_reopens() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.db");
        let id = {
            let s = Store::open(&p).unwrap();
            let id = s.create_meeting("live").unwrap();
            s.insert_segment(&seg(id, None, 0.0, "kept")).unwrap();
            id
        };
        let s = Store::open(&p).unwrap();
        assert_eq!(s.segments(id).unwrap()[0].text, "kept");
    }

    #[test]
    fn refuses_database_from_the_future() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x.db");
        Connection::open(&p).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(Store::open(&p).is_err());
    }
}
