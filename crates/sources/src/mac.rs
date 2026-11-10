//! The Mac's own databases, read-only: Messages, Calendar, Notes, Contacts.
//! iMessage messages are also read from `attributedBody`.
use crate::{BoxFuture, Item, Source, SourceError, clip};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Seconds between 1970-01-01 and Apple's epoch, 2001-01-01.
const APPLE_EPOCH_S: i64 = 978_307_200;

pub fn apple_s_to_ms(s: f64) -> i64 {
    ((s as i64) + APPLE_EPOCH_S) * 1000
}
fn ms_to_apple_s(ms: i64) -> i64 {
    ms / 1000 - APPLE_EPOCH_S
}
/// Messages stores nanoseconds since 2001 on current macOS and seconds on old ones.
pub fn message_date_to_ms(d: i64) -> i64 {
    if d > 10_000_000_000 {
        d / 1_000_000 + APPLE_EPOCH_S * 1000
    } else {
        (d + APPLE_EPOCH_S) * 1000
    }
}
fn ms_to_message_date(ms: i64) -> i64 {
    (ms - APPLE_EPOCH_S * 1000) * 1_000_000
}

/// Open read-only. A file that exists but cannot be opened means Full Disk Access is missing.
pub fn open_ro(path: &Path, what: &str) -> Result<Connection, SourceError> {
    match std::fs::File::open(path) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(SourceError::NeedsAccess(what.into()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SourceError::NotConnected(format!("no {what} on this Mac")));
        }
        Err(e) => return Err(SourceError::Failed(e.to_string())),
        Ok(_) => {}
    }
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| SourceError::Failed(format!("{what}: {e}")))?;
    let _ = c.busy_timeout(std::time::Duration::from_secs(2));
    Ok(c)
}

fn sql_err(what: &str) -> impl Fn(rusqlite::Error) -> SourceError + '_ {
    move |e| SourceError::Failed(format!("{what} changed shape or could not be read ({e})"))
}

/// The plain text inside an `NSAttributedString` typedstream: the bytes after the `NSString` class
/// name, a 5-byte header, then a length (one byte, or 0x81 and a little-endian u16, or 0x82 and a u32).
pub fn attributed_body_text(blob: &[u8]) -> Option<String> {
    let at = blob.windows(8).position(|w| w == b"NSString")? + 8;
    let rest = blob.get(at + 5..)?;
    let (len, start) = match *rest.first()? {
        0x81 => (u16::from_le_bytes([*rest.get(1)?, *rest.get(2)?]) as usize, 3),
        0x82 => (u32::from_le_bytes([*rest.get(1)?, *rest.get(2)?, *rest.get(3)?, *rest.get(4)?]) as usize, 5),
        n => (n as usize, 1),
    };
    let bytes = rest.get(start..start + len)?;
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Phone numbers compared by their last nine digits, so "+1 (415) 555-0100" matches "4155550100".
pub fn phone_key(s: &str) -> Option<String> {
    let d: String = s.chars().filter(char::is_ascii_digit).collect();
    (d.len() >= 7).then(|| d[d.len().saturating_sub(9)..].to_string())
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Contact {
    pub name: String,
    pub phones: Vec<String>,
    pub emails: Vec<String>,
}

/// Names for phone numbers and addresses.
#[derive(Debug, Clone, Default)]
pub struct Names(HashMap<String, String>);

impl Names {
    pub fn from_contacts(cs: &[Contact]) -> Self {
        let mut m = HashMap::new();
        for c in cs {
            for p in &c.phones {
                if let Some(k) = phone_key(p) {
                    m.insert(k, c.name.clone());
                }
            }
            for e in &c.emails {
                m.insert(e.to_lowercase(), c.name.clone());
            }
        }
        Self(m)
    }
    pub fn name_for(&self, handle: &str) -> Option<String> {
        self.0
            .get(&handle.to_lowercase())
            .or_else(|| phone_key(handle).and_then(|k| self.0.get(&k)))
            .cloned()
    }
}

/// Everyone in the Contacts database at `path`.
pub fn contacts(path: &Path) -> Result<Vec<Contact>, SourceError> {
    let db = open_ro(path, "Contacts")?;
    let e = sql_err("Contacts");
    let mut st = db
        .prepare(
            "SELECT Z_PK, ZFIRSTNAME, ZLASTNAME, ZORGANIZATION FROM ZABCDRECORD \
             WHERE ZFIRSTNAME IS NOT NULL OR ZLASTNAME IS NOT NULL OR ZORGANIZATION IS NOT NULL",
        )
        .map_err(&e)?;
    let rows: Vec<(i64, Contact)> = st
        .query_map([], |r| {
            let parts: Vec<String> = [r.get::<_, Option<String>>(1)?, r.get(2)?].into_iter().flatten().collect();
            let name = if parts.is_empty() { r.get::<_, Option<String>>(3)?.unwrap_or_default() } else { parts.join(" ") };
            Ok((r.get(0)?, Contact { name, ..Default::default() }))
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)?;
    let mut by_id: HashMap<i64, Contact> = rows.into_iter().collect();
    for (sql, phone) in [
        ("SELECT ZOWNER, ZFULLNUMBER FROM ZABCDPHONENUMBER", true),
        ("SELECT ZOWNER, ZADDRESS FROM ZABCDEMAILADDRESS", false),
    ] {
        let mut st = db.prepare(sql).map_err(&e)?;
        let it = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)))
            .map_err(&e)?;
        for (owner, v) in it.flatten() {
            if let (Some(c), Some(v)) = (by_id.get_mut(&owner), v) {
                if phone { c.phones.push(v) } else { c.emails.push(v) }
            }
        }
    }
    let mut v: Vec<Contact> = by_id.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}

/// Incoming messages since `since_ms`, newest first. `text` first, `attributedBody` when it is empty.
pub fn messages(path: &Path, since_ms: i64, names: &Names) -> Result<Vec<Item>, SourceError> {
    let db = open_ro(path, "Messages")?;
    let e = sql_err("Messages");
    let mut st = db
        .prepare(
            "SELECT m.ROWID, m.text, m.attributedBody, m.date, m.is_read, h.id, c.display_name \
             FROM message m \
             LEFT JOIN handle h ON h.ROWID = m.handle_id \
             LEFT JOIN chat_message_join j ON j.message_id = m.ROWID \
             LEFT JOIN chat c ON c.ROWID = j.chat_id \
             WHERE m.date > ?1 AND m.is_from_me = 0 \
             ORDER BY m.date DESC LIMIT 200",
        )
        .map_err(&e)?;
    let rows = st
        .query_map([ms_to_message_date(since_ms)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<bool>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(&e)?;
    let mut out = Vec::new();
    for (id, text, body, date, read, handle, chat) in rows.flatten() {
        let text = text
            .filter(|t| !t.trim().is_empty() && t != "\u{fffc}")
            .or_else(|| body.as_deref().and_then(attributed_body_text));
        let Some(text) = text else { continue };
        let handle = handle.unwrap_or_default();
        let who = names.name_for(&handle).unwrap_or_else(|| handle.clone());
        out.push(Item {
            source: "imessage".into(),
            id: id.to_string(),
            who,
            place: chat.filter(|c| !c.is_empty()).unwrap_or_else(|| "Messages".into()),
            when_ms: message_date_to_ms(date),
            text: clip(&text, 600),
            link: None,
            unread: read == Some(false),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub calendar: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    pub location: String,
    pub notes: String,
    pub attendees: Vec<String>,
}

/// Events that overlap `from_ms..to_ms`, soonest first, with their attendees' addresses.
pub fn events(path: &Path, from_ms: i64, to_ms: i64) -> Result<Vec<Event>, SourceError> {
    let db = open_ro(path, "Calendar")?;
    let e = sql_err("Calendar");
    let mut st = db
        .prepare(
            "SELECT ci.ROWID, ci.summary, COALESCE(c.title, ''), ci.start_date, ci.end_date, ci.all_day, \
                    COALESCE(l.title, ''), COALESCE(ci.description, '') \
             FROM CalendarItem ci JOIN Calendar c ON ci.calendar_id = c.ROWID \
             LEFT JOIN Location l ON l.ROWID = ci.location_id \
             WHERE ci.end_date >= ?1 AND ci.start_date <= ?2 AND ci.summary IS NOT NULL AND ci.hidden = 0 \
             ORDER BY ci.start_date ASC LIMIT 100",
        )
        .map_err(&e)?;
    let mut out: Vec<Event> = st
        .query_map([ms_to_apple_s(from_ms), ms_to_apple_s(to_ms)], |r| {
            Ok(Event {
                id: r.get(0)?,
                title: r.get(1)?,
                calendar: r.get(2)?,
                start_ms: apple_s_to_ms(r.get(3)?),
                end_ms: apple_s_to_ms(r.get(4)?),
                all_day: r.get::<_, Option<bool>>(5)?.unwrap_or(false),
                location: r.get(6)?,
                notes: r.get(7)?,
                attendees: Vec::new(),
            })
        })
        .map_err(&e)?
        .collect::<Result<_, _>>()
        .map_err(&e)?;
    // Attendees are optional: an older schema without the table still gives events.
    if let Ok(mut st) = db.prepare("SELECT email FROM Participant WHERE owner_id = ?1 AND email IS NOT NULL") {
        for ev in &mut out {
            ev.attendees = st
                .query_map([ev.id], |r| r.get::<_, String>(0))
                .map(|it| it.flatten().collect())
                .unwrap_or_default();
        }
    }
    Ok(out)
}

/// Plain text from a note's gzip-compressed protobuf body (their `_extract_text_from_zdata`).
pub fn note_text(zdata: &[u8]) -> String {
    let mut raw = Vec::new();
    if flate2::read::GzDecoder::new(zdata).read_to_end(&mut raw).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&raw);
    let cleaned: String = text
        .chars()
        .map(|c| if (c.is_control() && c != '\n') || c == '\u{fffd}' { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Notes changed since `since_ms`, newest first.
pub fn notes(path: &Path, since_ms: i64) -> Result<Vec<Item>, SourceError> {
    let db = open_ro(path, "Notes")?;
    let e = sql_err("Notes");
    let cols: Vec<String> = db
        .prepare("PRAGMA table_info(ZICCLOUDSYNCINGOBJECT)")
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(1)).map(|it| it.flatten().collect()))
        .map_err(&e)?;
    let pick = |names: &[&str]| -> String {
        let have: Vec<String> = names.iter().filter(|n| cols.iter().any(|c| c == *n)).map(|n| format!("n.{n}")).collect();
        if have.is_empty() { "NULL".into() } else { format!("COALESCE({})", [have, vec!["NULL".into()]].concat().join(", ")) }
    };
    let (title, modified, snippet) = (
        pick(&["ZTITLE1", "ZTITLE"]),
        pick(&["ZMODIFICATIONDATE1", "ZMODIFICATIONDATE"]),
        pick(&["ZSNIPPET"]),
    );
    let sql = format!(
        "SELECT n.Z_PK, {title}, {modified}, {snippet}, d.ZDATA FROM ZICCLOUDSYNCINGOBJECT n \
         JOIN ZICNOTEDATA d ON d.ZNOTE = n.Z_PK WHERE {modified} > ?1 ORDER BY {modified} DESC LIMIT 50"
    );
    let mut st = db.prepare(&sql).map_err(&e)?;
    let rows = st
        .query_map([ms_to_apple_s(since_ms)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<Vec<u8>>>(4)?,
            ))
        })
        .map_err(&e)?;
    Ok(rows
        .flatten()
        .map(|(id, title, modified, snippet, data)| {
            let body = data.as_deref().map(note_text).filter(|t| !t.is_empty()).or(snippet).unwrap_or_default();
            Item {
                source: "notes".into(),
                id: id.to_string(),
                who: "You".into(),
                place: title.unwrap_or_else(|| "Note".into()),
                when_ms: apple_s_to_ms(modified.unwrap_or(0.0)),
                text: clip(&body, 400),
                link: None,
                unread: false,
            }
        })
        .collect())
}

fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, SourceError> + Send + 'static) -> BoxFuture<'static, Result<T, SourceError>> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .unwrap_or_else(|e| Err(SourceError::Failed(e.to_string())))
    })
}

/// iMessage, with names from Contacts when Contacts can be read.
pub struct Messages {
    pub db: PathBuf,
    pub contacts: Vec<PathBuf>,
}

pub fn all_contacts(paths: &[PathBuf]) -> Vec<Contact> {
    paths.iter().filter_map(|p| contacts(p).ok()).flatten().collect()
}

impl Source for Messages {
    fn id(&self) -> &str {
        "imessage"
    }
    fn label(&self) -> &str {
        "Messages"
    }
    fn fetch(&self, since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let (db, contacts) = (self.db.clone(), self.contacts.clone());
        blocking(move || messages(&db, since_ms, &Names::from_contacts(&all_contacts(&contacts))))
    }
}

/// Upcoming events (now to 24 hours ahead), whatever `since` is: the calendar says what is next.
pub struct Calendar {
    pub db: PathBuf,
}

impl Source for Calendar {
    fn id(&self) -> &str {
        "calendar"
    }
    fn label(&self) -> &str {
        "Calendar"
    }
    fn fetch(&self, _since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let db = self.db.clone();
        blocking(move || {
            let now = crate::now_ms();
            Ok(events(&db, now, now + 24 * 3_600_000)?
                .into_iter()
                .map(|e| Item {
                    source: "calendar".into(),
                    id: format!("{}@{}", e.id, e.start_ms),
                    who: e.attendees.join(", "),
                    place: e.calendar.clone(),
                    when_ms: e.start_ms,
                    text: if e.location.is_empty() { e.title } else { format!("{} ({})", e.title, e.location) },
                    link: None,
                    unread: false,
                })
                .collect())
        })
    }
}

pub struct Notes {
    pub db: PathBuf,
}

impl Source for Notes {
    fn id(&self) -> &str {
        "notes"
    }
    fn label(&self) -> &str {
        "Notes"
    }
    fn fetch(&self, since_ms: i64) -> BoxFuture<'static, Result<Vec<Item>, SourceError>> {
        let db = self.db.clone();
        blocking(move || notes(&db, since_ms))
    }
}
