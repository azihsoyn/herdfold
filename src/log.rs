//! The reading log: what happened in each sitting with a book, kept as one
//! JSON Lines file a session in `$XDG_DATA_HOME/<name>/sessions/` (default
//! `~/.local/share/<name>/sessions/`), one `Record` a line. `herdfold log`
//! lists, shows, exports and imports them, and reopens a book where a
//! session left it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::NAME;
pub use crate::api::{Book, Place};
use crate::api::{EventData as Event, Move};
use crate::cli::CliError;
use crate::layout::Pos;

/// One line of a session's log: an event of the socket API, as it was
/// sent, with the session it belongs to and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Record {
    /// The session the record belongs to.
    pub session: String,
    /// When it happened, RFC 3339 in UTC.
    pub time: String,
    #[serde(flatten)]
    pub event: Event,
}

pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join(NAME).join("sessions"))
}

/// The log of the session going on: the events the reader sends, kept
/// as they happen. The file is made with the session's first record.
pub struct Log {
    session: String,
    file: Option<fs::File>,
}

impl Log {
    pub fn new() -> Self {
        Self {
            session: new_session_id(),
            file: None,
        }
    }

    /// Keeps `event`, if it is one the log keeps.
    pub fn write(&mut self, event: &Event) {
        if !event.logged() {
            return;
        }
        if self.file.is_none() {
            self.file = dir().and_then(|d| {
                fs::create_dir_all(&d).ok()?;
                fs::File::create_new(d.join(format!("{}.jsonl", self.session))).ok()
            });
        }
        let Some(file) = &mut self.file else {
            return;
        };
        let record = Record {
            session: self.session.clone(),
            time: rfc3339(SystemTime::now()),
            event: event.clone(),
        };
        if let Ok(line) = serde_json::to_string(&record) {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// A session id that sorts by when it began: `20261005T120000Z-1a2b3c`.
fn new_session_id() -> String {
    let now = SystemTime::now();
    let stamp: String = rfc3339(now)
        .chars()
        .filter(|c| *c != '-' && *c != ':')
        .collect();
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{stamp}-{:06x}", (nanos ^ std::process::id()) & 0xff_ffff)
}

/// `t` as RFC 3339 in UTC, to the second.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Every record in `path`; lines that are not records are passed over.
fn read(path: &std::path::Path) -> Vec<Record> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| parse(&l))
        .collect()
}

/// A record from a line, as written now or by 0.3.0, whose pages turned
/// were `page_shown` with only a place.
fn parse(line: &str) -> Option<Record> {
    let mut v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["type"] == "page_shown" && v.get("left").is_none() {
        v["type"] = "reader_moved".into();
    }
    serde_json::from_value(v).ok()
}

/// Every session's records, oldest session first.
fn sessions() -> Vec<Vec<Record>> {
    let Some(dir) = dir() else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.retain(|p| p.extension().is_some_and(|e| e == "jsonl"));
    paths.sort();
    paths
        .iter()
        .map(|p| read(p))
        .filter(|r| !r.is_empty())
        .collect()
}

/// Every session, summed up, oldest first.
pub fn summaries() -> Vec<Summary> {
    sessions().iter().filter_map(|r| summarize(r)).collect()
}

/// One session, summed up.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct Summary {
    pub session: String,
    pub book: Book,
    pub started: String,
    /// When the last record was written.
    pub ended: String,
    /// Whether the book was closed, rather than the session cut off.
    pub closed: bool,
    pub from: Place,
    pub to: Place,
    pub pages_read: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    pub notes: usize,
    pub bookmarks: usize,
    pub searches: usize,
    pub questions: usize,
    /// Whether the last page was reached.
    pub finished: bool,
}

pub fn summarize(records: &[Record]) -> Option<Summary> {
    let first = records.first()?;
    let Event::SessionStarted { book, at, .. } = &first.event else {
        return None;
    };
    let mut s = Summary {
        session: first.session.clone(),
        book: book.clone(),
        started: first.time.clone(),
        ended: first.time.clone(),
        closed: false,
        from: at.clone(),
        to: at.clone(),
        pages_read: 1,
        seconds: None,
        notes: 0,
        bookmarks: 0,
        searches: 0,
        questions: 0,
        finished: false,
    };
    let mut pages = BTreeSet::from([at.page]);
    for r in records {
        s.ended = r.time.clone();
        match &r.event {
            Event::SessionStarted { .. } => {}
            Event::ReaderMoved { at, .. } => {
                pages.insert(at.page);
                s.to = at.clone();
            }
            Event::BookmarkAdded { .. } => s.bookmarks += 1,
            Event::NoteAdded { .. } => s.notes += 1,
            Event::SearchDone { .. } => s.searches += 1,
            Event::QuestionAsked { .. } => s.questions += 1,
            Event::BookFinished { .. } => s.finished = true,
            Event::PageShown { .. }
            | Event::ReaderClosed
            | Event::SettingsChanged { .. }
            | Event::BookmarkChanged { .. }
            | Event::BookmarkRemoved { .. }
            | Event::NoteChanged { .. }
            | Event::NoteRemoved { .. } => {}
            Event::SessionEnded {
                at,
                pages_read,
                seconds,
            } => {
                s.closed = true;
                s.to = at.clone();
                s.seconds = Some(*seconds);
                pages.insert(at.page);
                s.pages_read = (*pages_read).max(1);
            }
        }
    }
    if !s.closed {
        s.pages_read = pages.len();
    }
    Some(s)
}

/// Seconds a page usually takes to read: the median time between turns to
/// the next page, from this book's sessions if there are enough, else from
/// every book's. Turns after a long pause (the book left open) do not count.
pub fn pace(book: Option<&str>) -> Option<f64> {
    const LONGEST: f64 = 600.0;
    const ENOUGH: usize = 10;
    let all = sessions();
    let samples = |only: Option<&str>| {
        let mut out = Vec::new();
        for records in &all {
            let of_book =
                summarize(records).is_some_and(|s| only.is_none() || s.book.key.as_deref() == only);
            if !of_book {
                continue;
            }
            let mut last: Option<(usize, f64)> = None;
            for r in records {
                // Pace is from turning, not from jumps.
                let at = match &r.event {
                    Event::SessionStarted { at, .. } => at,
                    Event::ReaderMoved { at, how } => {
                        if *how != Move::Turn {
                            last = None;
                            continue;
                        }
                        at
                    }
                    _ => continue,
                };
                let Some(t) = seconds_of(&r.time) else {
                    continue;
                };
                if let Some((page, then)) = last
                    && (1..=2).contains(&at.page.saturating_sub(page))
                    && at.page > page
                {
                    let each = (t - then) / (at.page - page) as f64;
                    if each > 0.0 && each <= LONGEST {
                        out.push(each);
                    }
                }
                last = Some((at.page, t));
            }
        }
        out
    };
    let mut s = samples(book);
    if s.len() < ENOUGH {
        s = samples(None);
    }
    if s.len() < ENOUGH {
        return None;
    }
    s.sort_by(f64::total_cmp);
    Some(s[s.len() / 2])
}

/// Seconds since 1970 of an RFC 3339 time in UTC, as `rfc3339` writes them.
fn seconds_of(time: &str) -> Option<f64> {
    let n = |r: std::ops::Range<usize>| time.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // A civil date to days since 1970-01-01 (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hh * 3600 + mm * 60 + ss) as f64)
}

/// A rough length of time, as a reader would say it.
pub fn duration(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    match minutes {
        0 => "under a minute".to_string(),
        m if m < 60 => format!("about {m} min"),
        m => match m % 60 {
            0 => format!("about {} h", m / 60),
            r => format!("about {} h {r} min", m / 60),
        },
    }
}

fn print(id: &str, result: serde_json::Value) {
    println!(
        "{}",
        serde_json::json!({ "id": format!("cli:{id}"), "result": result })
    );
}

/// `herdfold log list [--book FILE]`: the sessions, newest first.
pub fn list(book: Option<PathBuf>) -> Result<(), CliError> {
    let key = book.map(canonical).transpose()?;
    let mut all: Vec<Summary> = sessions()
        .iter()
        .filter_map(|r| summarize(r))
        .filter(|s| key.is_none() || s.book.key == key)
        .collect();
    all.reverse();
    print(
        "log:list",
        serde_json::json!({ "type": "sessions", "sessions": all }),
    );
    Ok(())
}

fn find(session: &str) -> Result<Vec<Record>, CliError> {
    let path = dir()
        .map(|d| d.join(format!("{session}.jsonl")))
        .filter(|p| p.is_file())
        .ok_or_else(|| CliError::new("session_not_found", format!("no session {session}")))?;
    Ok(read(&path))
}

/// `herdfold log show SESSION`: its summary and every record.
pub fn show(session: &str) -> Result<(), CliError> {
    let records = find(session)?;
    print(
        "log:show",
        serde_json::json!({
            "type": "session",
            "summary": summarize(&records),
            "records": records,
        }),
    );
    Ok(())
}

/// `herdfold log export [--book FILE]`: every record, as JSON Lines on
/// stdout, oldest first.
pub fn export(book: Option<PathBuf>) -> Result<(), CliError> {
    let key = book.map(canonical).transpose()?;
    let mut out = std::io::stdout().lock();
    for records in sessions() {
        let of_book = summarize(&records).is_some_and(|s| key.is_none() || s.book.key == key);
        if !of_book {
            continue;
        }
        for r in records {
            let line =
                serde_json::to_string(&r).map_err(|e| CliError::new("internal", e.to_string()))?;
            writeln!(out, "{line}").map_err(CliError::io)?;
        }
    }
    Ok(())
}

/// `herdfold log import FILE|-`: records from JSON Lines, as `export`
/// writes them. Sessions already kept are left as they are.
pub fn import(file: PathBuf) -> Result<(), CliError> {
    let text = if file.as_os_str() == "-" {
        std::io::read_to_string(std::io::stdin()).map_err(CliError::io)?
    } else {
        fs::read_to_string(&file).map_err(CliError::io)?
    };
    let mut by_session: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let r: Record = serde_json::from_str(line)
            .map_err(|e| CliError::new("invalid_record", format!("line {}: {e}", n + 1)))?;
        let safe = !r.session.is_empty()
            && r.session
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !safe {
            return Err(CliError::new(
                "invalid_record",
                format!("line {}: unusable session id {:?}", n + 1, r.session),
            ));
        }
        by_session.entry(r.session.clone()).or_default().push(r);
    }
    let dir = dir().ok_or_else(|| CliError::new("no_home", "nowhere to keep the log"))?;
    fs::create_dir_all(&dir).map_err(CliError::io)?;
    let (mut imported, mut skipped) = (0, 0);
    for (session, records) in by_session {
        let path = dir.join(format!("{session}.jsonl"));
        let Ok(mut f) = fs::File::create_new(&path) else {
            skipped += 1;
            continue;
        };
        for r in records {
            let line =
                serde_json::to_string(&r).map_err(|e| CliError::new("internal", e.to_string()))?;
            writeln!(f, "{line}").map_err(CliError::io)?;
        }
        imported += 1;
    }
    print(
        "log:import",
        serde_json::json!({ "type": "imported", "imported": imported, "skipped": skipped }),
    );
    Ok(())
}

/// The book and place a session left off at, to open it there again.
pub fn resume_point(session: &str) -> Result<(Book, Pos), CliError> {
    let records = find(session)?;
    let s = summarize(&records)
        .ok_or_else(|| CliError::new("invalid_session", format!("{session} has no start")))?;
    if s.book.key.is_none() {
        return Err(CliError::new(
            "not_a_file",
            "that session read stdin, which cannot be opened again",
        ));
    }
    Ok((s.book, s.to.pos()))
}

fn canonical(p: PathBuf) -> Result<String, CliError> {
    fs::canonicalize(&p)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(CliError::io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::Format;
    use crate::marks::Anchor;

    fn place(page: usize) -> Place {
        Place {
            line: page * 10,
            offset: 0,
            page,
            pages: 20,
            chapter: None,
        }
    }

    #[test]
    fn dates_are_rfc3339() {
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_791_201_600);
        assert_eq!(rfc3339(t), "2026-10-05T12:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(seconds_of("2026-10-05T12:00:00Z"), Some(1_791_201_600.0));
        assert_eq!(seconds_of("2024-02-29T00:00:01Z"), Some(1_709_164_801.0));
    }

    #[test]
    fn durations_read_as_spoken() {
        assert_eq!(duration(20.0), "under a minute");
        assert_eq!(duration(20.0 * 60.0), "about 20 min");
        assert_eq!(duration(80.0 * 60.0), "about 1 h 20 min");
        assert_eq!(duration(120.0 * 60.0), "about 2 h");
    }

    #[test]
    fn records_are_flat_json_lines() {
        let r = Record {
            session: "s".into(),
            time: "t".into(),
            event: Event::SearchDone {
                query: "cat".into(),
                finds: 2,
            },
        };
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(
            line,
            r#"{"session":"s","time":"t","type":"search_done","query":"cat","finds":2}"#
        );
        assert_eq!(serde_json::from_str::<Record>(&line).unwrap(), r);
    }

    #[test]
    fn logs_written_by_0_3_0_still_read() {
        let old = r#"{"session":"s","time":"t","type":"page_shown","at":{"line":1,"offset":0,"page":2,"pages":9}}"#;
        let r = parse(old).unwrap();
        assert!(matches!(
            r.event,
            Event::ReaderMoved {
                how: Move::Turn,
                ..
            }
        ));
        let old = r#"{"session":"s","time":"t","type":"finished","at":{"line":1,"offset":0,"page":9,"pages":9}}"#;
        assert!(matches!(
            parse(old).unwrap().event,
            Event::BookFinished { .. }
        ));
    }

    #[test]
    fn a_session_is_summed_up() {
        let book = Book {
            key: Some("/b.md".into()),
            title: "B".into(),
            format: Format::Md,
        };
        let rec = |time: &str, event| Record {
            session: "s".into(),
            time: time.into(),
            event,
        };
        let records = [
            rec(
                "1",
                Event::SessionStarted {
                    book,
                    at: place(3),
                    version: "x".into(),
                },
            ),
            rec(
                "2",
                Event::ReaderMoved {
                    at: place(4),
                    how: Move::Turn,
                },
            ),
            rec(
                "3",
                Event::NoteAdded {
                    at: place(4),
                    anchor: Anchor::Page,
                    text: "n".into(),
                    quote: None,
                    by: crate::marks::Author::Reader,
                    question: None,
                },
            ),
            rec(
                "4",
                Event::ReaderMoved {
                    at: place(5),
                    how: Move::Jump,
                },
            ),
        ];
        let s = summarize(&records).unwrap();
        assert!(!s.finished);
        assert_eq!((s.from.page, s.to.page, s.pages_read), (3, 5, 3));
        assert_eq!((s.notes, s.closed, s.ended.as_str()), (1, false, "4"));
    }
}
