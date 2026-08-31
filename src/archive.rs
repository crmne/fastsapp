//! The message archive: every chat and message this device has seen.
//!
//! WhatsApp replays a chat's history once, when the device is linked, and
//! streams messages live after that; a client that keeps nothing shows an
//! empty window on its second start. One SQLite file holds it all. The raw
//! protobuf of each message is kept beside the decoded row, because the
//! keys to fetch an attachment live in it and a picture may be asked for
//! weeks later.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Chat, ChatKind, Contact, Content, Delivery, LastMessage, Message};

/// A sticker the phone lists among its recently used ones: its file hash,
/// the `StickerMetadata` protobuf holding the keys to fetch it, when it was
/// last sent, and the file once fetched.
#[derive(Clone, Debug)]
pub struct PhoneSticker {
    pub hash: String,
    pub raw: Vec<u8>,
    pub last_used: i64,
    pub path: Option<std::path::PathBuf>,
}

/// A sticker that passed through a chat and is on disk: when it last did,
/// its file, and the raw message it came in.
#[derive(Clone, Debug)]
pub struct ArchivedSticker {
    pub last_used: i64,
    pub path: std::path::PathBuf,
    pub raw: Option<Vec<u8>>,
}

pub struct Archive {
    connection: Connection,
}

pub type Result<T> = std::result::Result<T, rusqlite::Error>;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chats (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    last_activity INTEGER NOT NULL DEFAULT 0,
    unread INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    muted_until INTEGER
);
CREATE TABLE IF NOT EXISTS messages (
    chat TEXT NOT NULL,
    id TEXT NOT NULL,
    sender TEXT NOT NULL,
    sender_name TEXT,
    from_me INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    content TEXT NOT NULL,
    status INTEGER NOT NULL DEFAULT 0,
    quoted TEXT,
    reactions TEXT NOT NULL DEFAULT '[]',
    edited INTEGER NOT NULL DEFAULT 0,
    raw BLOB,
    PRIMARY KEY (chat, id)
);
CREATE INDEX IF NOT EXISTS messages_by_time ON messages (chat, timestamp);
CREATE TABLE IF NOT EXISTS contacts (
    id TEXT PRIMARY KEY,
    full_name TEXT,
    push_name TEXT
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS lids (
    lid TEXT PRIMARY KEY,
    pn TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS stickers (
    hash TEXT PRIMARY KEY,
    raw BLOB NOT NULL,
    last_used INTEGER NOT NULL DEFAULT 0,
    weight REAL NOT NULL DEFAULT 0,
    path TEXT
);
";

const CHAT_COLUMNS: &str =
    "c.id, c.name, c.kind, c.last_activity, c.unread, c.archived, c.pinned, c.muted_until,
                    m.from_me, m.sender_name, m.content, m.status, m.sender, c.participants, c.read_only";

/// Columns added after the first release; each is created when missing so
/// an archive from an earlier version keeps working.
const MIGRATIONS: &[(&str, &str, &str)] = &[
    ("messages", "thumbnail", "BLOB"),
    ("messages", "mentions", "TEXT NOT NULL DEFAULT '[]'"),
    ("chats", "participants", "TEXT NOT NULL DEFAULT '[]'"),
    ("chats", "read_only", "INTEGER NOT NULL DEFAULT 0"),
    ("messages", "forwarded", "INTEGER NOT NULL DEFAULT 0"),
    ("messages", "delivered_at", "INTEGER"),
    ("messages", "read_at", "INTEGER"),
];
const CHAT_JOIN: &str = "FROM chats c
             LEFT JOIN messages m ON m.chat = c.id AND m.rowid = (
                 SELECT rowid FROM messages WHERE chat = c.id ORDER BY timestamp DESC, rowid DESC LIMIT 1
             )";

fn chat_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chat> {
    let content: Option<String> = row.get(10)?;
    let last = match content {
        Some(content) => {
            let content: Content = serde_json::from_str(&content).unwrap_or(Content::Unsupported {
                what: "unreadable".into(),
            });
            Some(LastMessage {
                from_me: row.get(8)?,
                sender: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
                sender_name: row.get(9)?,
                summary: content.summary(),
                status: status_from_rank(row.get(11)?),
            })
        }
        None => None,
    };
    let kind: String = row.get(2)?;
    let participants: String = row.get(13)?;
    Ok(Chat {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: kind_from_name(&kind),
        last_activity: row.get(3)?,
        unread: row.get(4)?,
        archived: row.get(5)?,
        pinned: row.get(6)?,
        muted_until: row.get(7)?,
        last,
        participants: serde_json::from_str(&participants).unwrap_or_default(),
        read_only: row.get(14)?,
    })
}

fn status_rank(status: Delivery) -> i64 {
    match status {
        Delivery::None => 0,
        Delivery::Pending => 1,
        Delivery::Sent => 2,
        Delivery::Delivered => 3,
        Delivery::Read => 4,
        Delivery::Played => 5,
        Delivery::Failed => 6,
    }
}

/// The column that keeps when a stage was reached, for the stages worth
/// remembering.
fn stamp_column(status: Delivery) -> Option<&'static str> {
    match status {
        Delivery::Delivered => Some("delivered_at"),
        Delivery::Read | Delivery::Played => Some("read_at"),
        _ => None,
    }
}

fn status_from_rank(rank: i64) -> Delivery {
    match rank {
        1 => Delivery::Pending,
        2 => Delivery::Sent,
        3 => Delivery::Delivered,
        4 => Delivery::Read,
        5 => Delivery::Played,
        6 => Delivery::Failed,
        _ => Delivery::None,
    }
}

fn kind_name(kind: ChatKind) -> &'static str {
    match kind {
        ChatKind::Direct => "direct",
        ChatKind::Group => "group",
        ChatKind::Broadcast => "broadcast",
    }
}

fn kind_from_name(name: &str) -> ChatKind {
    match name {
        "group" => ChatKind::Group,
        "broadcast" => ChatKind::Broadcast,
        _ => ChatKind::Direct,
    }
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let connection = Connection::open(path)?;
        Self::prepare(connection)
    }

    pub fn in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> Result<Self> {
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        connection.execute_batch(SCHEMA)?;
        for (table, column, definition) in MIGRATIONS {
            let exists = connection
                .prepare(&format!("PRAGMA table_info({table})"))?
                .query_map([], |row| row.get::<_, String>(1))?
                .any(|name| name.as_deref() == Ok(*column));
            if !exists {
                connection.execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN {column} {definition}"
                ))?;
            }
        }
        Ok(Self { connection })
    }

    /// Creates the chat if it is new, and renames it if `name` is better
    /// than what is stored (a phone number is never better than a name).
    pub fn upsert_chat(&self, chat: &Chat) -> Result<()> {
        self.connection.execute(
            "INSERT INTO chats (id, name, kind, last_activity, unread, archived, pinned, muted_until)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                last_activity = MAX(last_activity, excluded.last_activity),
                archived = excluded.archived,
                pinned = excluded.pinned,
                muted_until = excluded.muted_until",
            params![
                chat.id,
                chat.name,
                kind_name(chat.kind),
                chat.last_activity,
                chat.unread,
                chat.archived,
                chat.pinned,
                chat.muted_until,
            ],
        )?;
        Ok(())
    }

    /// Makes sure a chat row exists without touching an existing one.
    pub fn ensure_chat(&self, id: &str, name: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR IGNORE INTO chats (id, name, kind) VALUES (?1, ?2, ?3)",
            params![id, name, kind_name(ChatKind::from_id(id))],
        )?;
        Ok(())
    }

    /// What WhatsApp said about a group: its subject, members, and whether
    /// we may post.
    pub fn set_group_info(
        &self,
        id: &str,
        name: Option<&str>,
        participants: &[String],
        read_only: bool,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET name = COALESCE(?2, name), participants = ?3, read_only = ?4 WHERE id = ?1",
            params![
                id,
                name,
                serde_json::to_string(participants).unwrap_or_else(|_| "[]".into()),
                read_only
            ],
        )?;
        Ok(())
    }

    pub fn rename_chat(&self, id: &str, name: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    pub fn set_archived(&self, id: &str, archived: bool) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET archived = ?2 WHERE id = ?1",
            params![id, archived],
        )?;
        Ok(())
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET pinned = ?2 WHERE id = ?1",
            params![id, pinned],
        )?;
        Ok(())
    }

    pub fn set_muted(&self, id: &str, until: Option<i64>) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET muted_until = ?2 WHERE id = ?1",
            params![id, until],
        )?;
        Ok(())
    }

    pub fn mark_read(&self, id: &str) -> Result<()> {
        self.connection
            .execute("UPDATE chats SET unread = 0 WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_unread(&self, id: &str, unread: u32) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET unread = ?2 WHERE id = ?1",
            params![id, unread],
        )?;
        Ok(())
    }

    /// Every chat, newest activity first, each with its last message.
    pub fn chats(&self) -> Result<Vec<Chat>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {CHAT_COLUMNS} {CHAT_JOIN} ORDER BY c.last_activity DESC"
        ))?;
        let rows = statement.query_map([], chat_from_row)?;
        rows.collect()
    }

    pub fn chat(&self, id: &str) -> Result<Option<Chat>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {CHAT_COLUMNS} {CHAT_JOIN} WHERE c.id = ?1"
        ))?;
        statement.query_row(params![id], chat_from_row).optional()
    }

    pub fn bump_unread(&self, id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE chats SET unread = unread + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// The newest `limit` messages from the other side, as (id, sender),
    /// for read receipts.
    pub fn unread_incoming(&self, chat: &str, limit: u32) -> Result<Vec<(String, String)>> {
        let mut statement = self.connection.prepare(
            "SELECT id, sender FROM messages WHERE chat = ?1 AND from_me = 0
             ORDER BY timestamp DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![chat, i64::from(limit)], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect()
    }

    /// Records where a message's attachment was saved.
    pub fn set_media_path(&self, chat: &str, id: &str, path: &Path) -> Result<Option<Message>> {
        self.put_media_path(chat, id, Some(path))
    }

    /// Forgets where an attachment was, so it is fetched again when
    /// wanted.
    pub fn clear_media_path(&self, chat: &str, id: &str) -> Result<Option<Message>> {
        self.put_media_path(chat, id, None)
    }

    fn put_media_path(&self, chat: &str, id: &str, path: Option<&Path>) -> Result<Option<Message>> {
        let Some(mut message) = self.message(chat, id)? else {
            return Ok(None);
        };
        let Some(media) = message.content.media_mut() else {
            return Ok(None);
        };
        media.path = path.map(Path::to_path_buf);
        self.set_content(chat, id, &message.content, message.edited)?;
        Ok(Some(message))
    }

    /// Every attachment on record with where its file was put.
    pub fn media_paths(&self) -> Result<Vec<(String, String, std::path::PathBuf)>> {
        let mut statement = self.connection.prepare(
            "SELECT chat, id, json_extract(content, '$.media.path') AS path
             FROM messages WHERE path IS NOT NULL",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                std::path::PathBuf::from(row.get::<_, String>(2)?),
            ))
        })?;
        rows.collect()
    }

    /// A privacy id and the phone number behind it, both as the bare user
    /// part.
    pub fn put_lid(&self, lid: &str, pn: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO lids (lid, pn) VALUES (?1, ?2) ON CONFLICT(lid) DO UPDATE SET pn = excluded.pn",
            params![lid, pn],
        )?;
        Ok(())
    }

    pub fn lids(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.connection.prepare("SELECT lid, pn FROM lids")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Stores a message, replacing an earlier copy while keeping whichever
    /// delivery status has travelled further, and bumps the chat's
    /// activity. `raw` is the protobuf, kept for attachments.
    pub fn insert_message(&self, message: &Message, raw: Option<&[u8]>) -> Result<()> {
        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT status FROM messages WHERE chat = ?1 AND id = ?2",
                params![message.chat, message.id],
                |row| row.get(0),
            )
            .optional()?;
        let status = match existing {
            Some(rank)
                if message.status != Delivery::Failed && rank > status_rank(message.status) =>
            {
                rank
            }
            _ => status_rank(message.status),
        };
        self.connection.execute(
            "INSERT INTO messages (chat, id, sender, sender_name, from_me, timestamp, content, status, quoted, reactions, edited, raw, thumbnail, mentions, forwarded, delivered_at, read_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(chat, id) DO UPDATE SET
                sender_name = COALESCE(excluded.sender_name, sender_name),
                content = excluded.content,
                status = excluded.status,
                quoted = COALESCE(excluded.quoted, quoted),
                reactions = excluded.reactions,
                edited = excluded.edited,
                raw = COALESCE(excluded.raw, raw),
                thumbnail = COALESCE(excluded.thumbnail, thumbnail),
                mentions = excluded.mentions,
                forwarded = excluded.forwarded,
                delivered_at = COALESCE(delivered_at, excluded.delivered_at),
                read_at = COALESCE(read_at, excluded.read_at)",
            params![
                message.chat,
                message.id,
                message.sender,
                message.sender_name,
                message.from_me,
                message.timestamp,
                serde_json::to_string(&message.content).unwrap_or_default(),
                status,
                message
                    .quoted
                    .as_ref()
                    .map(|quoted| serde_json::to_string(quoted).unwrap_or_default()),
                serde_json::to_string(&message.reactions).unwrap_or_default(),
                message.edited,
                raw,
                message.thumbnail.as_deref(),
                serde_json::to_string(&message.mentions).unwrap_or_default(),
                message.forwarded,
                message.delivered_at,
                message.read_at,
            ],
        )?;
        self.connection.execute(
            "UPDATE chats SET last_activity = MAX(last_activity, ?2) WHERE id = ?1",
            params![message.chat, message.timestamp],
        )?;
        Ok(())
    }

    /// The newest `limit` messages of a chat older than `before`, the time
    /// and id of a message (all of them when `None`), oldest first. Several
    /// messages can share a second (an album), so the boundary is the
    /// message itself, not its time.
    pub fn messages(
        &self,
        chat: &str,
        before: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let mut statement = self.connection.prepare(
            "SELECT id, sender, sender_name, from_me, timestamp, content, status, quoted, reactions, edited, thumbnail, mentions, forwarded, delivered_at, read_at
             FROM messages
             WHERE chat = ?1 AND (timestamp < ?2 OR (timestamp = ?2 AND rowid <
                 (SELECT rowid FROM messages WHERE chat = ?1 AND id = ?3)))
             ORDER BY timestamp DESC, rowid DESC
             LIMIT ?4",
        )?;
        let (before_time, before_id) = before.unwrap_or((i64::MAX, ""));
        let rows =
            statement.query_map(params![chat, before_time, before_id, limit as i64], |row| {
                let content: String = row.get(5)?;
                let quoted: Option<String> = row.get(7)?;
                let reactions: String = row.get(8)?;
                let mentions: String = row.get(11)?;
                Ok(Message {
                    id: row.get(0)?,
                    chat: chat.to_owned(),
                    sender: row.get(1)?,
                    sender_name: row.get(2)?,
                    from_me: row.get(3)?,
                    timestamp: row.get(4)?,
                    content: serde_json::from_str(&content).unwrap_or(Content::Unsupported {
                        what: "unreadable".into(),
                    }),
                    status: status_from_rank(row.get(6)?),
                    delivered_at: row.get(13)?,
                    read_at: row.get(14)?,
                    quoted: quoted.and_then(|quoted| serde_json::from_str(&quoted).ok()),
                    reactions: serde_json::from_str(&reactions).unwrap_or_default(),
                    edited: row.get(9)?,
                    mentions: serde_json::from_str(&mentions).unwrap_or_default(),
                    forwarded: row.get(12)?,
                    thumbnail: row.get(10)?,
                })
            })?;
        let mut messages: Vec<Message> = rows.collect::<Result<_>>()?;
        messages.reverse();
        Ok(messages)
    }

    /// Messages whose visible text — body, caption, file name, poll
    /// question, contact or place name — contains `needle`, newest first.
    /// Case-insensitive over ASCII, exact beyond it, like SQLite itself.
    pub fn search_messages(&self, needle: &str, limit: usize) -> Result<Vec<Message>> {
        let pattern = format!(
            "%{}%",
            needle
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut statement = self.connection.prepare(
            "SELECT chat, id, sender, sender_name, from_me, timestamp, content, status, quoted, reactions, edited, thumbnail, mentions, forwarded, delivered_at, read_at
             FROM messages
             WHERE json_valid(content) AND lower(
                     coalesce(json_extract(content, '$.text'), '') || char(10) ||
                     coalesce(json_extract(content, '$.caption'), '') || char(10) ||
                     coalesce(json_extract(content, '$.file_name'), '') || char(10) ||
                     coalesce(json_extract(content, '$.question'), '') || char(10) ||
                     coalesce(json_extract(content, '$.display_name'), '') || char(10) ||
                     coalesce(json_extract(content, '$.name'), '')
                 ) LIKE ?1 ESCAPE '\\'
             ORDER BY timestamp DESC, rowid DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![pattern, limit as i64], |row| {
            let chat: String = row.get(0)?;
            let content: String = row.get(6)?;
            let quoted: Option<String> = row.get(8)?;
            let reactions: String = row.get(9)?;
            let mentions: String = row.get(12)?;
            Ok(Message {
                id: row.get(1)?,
                chat,
                sender: row.get(2)?,
                sender_name: row.get(3)?,
                from_me: row.get(4)?,
                timestamp: row.get(5)?,
                content: serde_json::from_str(&content).unwrap_or(Content::Unsupported {
                    what: "unreadable".into(),
                }),
                status: status_from_rank(row.get(7)?),
                delivered_at: row.get(14)?,
                read_at: row.get(15)?,
                quoted: quoted.and_then(|quoted| serde_json::from_str(&quoted).ok()),
                reactions: serde_json::from_str(&reactions).unwrap_or_default(),
                edited: row.get(10)?,
                mentions: serde_json::from_str(&mentions).unwrap_or_default(),
                forwarded: row.get(13)?,
                thumbnail: row.get(11)?,
            })
        })?;
        let messages: Vec<Message> = rows.collect::<Result<_>>()?;
        Ok(messages)
    }

    /// Every message from `from` (inclusive) up to `before` (exclusive),
    /// oldest first, capped at `limit`.
    pub fn messages_range(
        &self,
        chat: &str,
        from: i64,
        before: (i64, &str),
        limit: usize,
    ) -> Result<Vec<Message>> {
        let mut statement = self.connection.prepare(
            "SELECT id, sender, sender_name, from_me, timestamp, content, status, quoted, reactions, edited, thumbnail, mentions, forwarded, delivered_at, read_at
             FROM messages
             WHERE chat = ?1 AND timestamp >= ?2 AND (timestamp < ?3 OR (timestamp = ?3 AND rowid <
                 (SELECT rowid FROM messages WHERE chat = ?1 AND id = ?4)))
             ORDER BY timestamp ASC, rowid ASC
             LIMIT ?5",
        )?;
        let rows = statement.query_map(
            params![chat, from, before.0, before.1, limit as i64],
            |row| {
                let content: String = row.get(5)?;
                let quoted: Option<String> = row.get(7)?;
                let reactions: String = row.get(8)?;
                let mentions: String = row.get(11)?;
                Ok(Message {
                    id: row.get(0)?,
                    chat: chat.to_owned(),
                    sender: row.get(1)?,
                    sender_name: row.get(2)?,
                    from_me: row.get(3)?,
                    timestamp: row.get(4)?,
                    content: serde_json::from_str(&content).unwrap_or(Content::Unsupported {
                        what: "unreadable".into(),
                    }),
                    status: status_from_rank(row.get(6)?),
                    delivered_at: row.get(13)?,
                    read_at: row.get(14)?,
                    quoted: quoted.and_then(|quoted| serde_json::from_str(&quoted).ok()),
                    reactions: serde_json::from_str(&reactions).unwrap_or_default(),
                    edited: row.get(9)?,
                    mentions: serde_json::from_str(&mentions).unwrap_or_default(),
                    forwarded: row.get(12)?,
                    thumbnail: row.get(10)?,
                })
            },
        )?;
        rows.collect()
    }

    /// Stickers that went through any chat and are on disk, newest first,
    /// for the picker: when they last passed, their file, and the raw
    /// message they came in, to tell the same sticker in two messages.
    pub fn recent_stickers(&self, limit: usize) -> Result<Vec<ArchivedSticker>> {
        let mut statement = self.connection.prepare(
            "SELECT json_extract(content, '$.media.path') AS path, MAX(timestamp), raw
             FROM messages
             WHERE json_extract(content, '$.kind') = 'sticker' AND path IS NOT NULL
             GROUP BY path
             ORDER BY 2 DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(ArchivedSticker {
                last_used: row.get(1)?,
                path: std::path::PathBuf::from(row.get::<_, String>(0)?),
                raw: row.get(2)?,
            })
        })?;
        Ok(rows
            .flatten()
            .filter(|sticker| sticker.path.exists())
            .collect())
    }

    /// Sticker messages whose file was never fetched, ours first and
    /// newest first, so the picker can fill itself in.
    pub fn stickers_without_file(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let mut statement = self.connection.prepare(
            "SELECT chat, id FROM messages
             WHERE json_extract(content, '$.kind') = 'sticker'
               AND json_extract(content, '$.media.path') IS NULL
               AND raw IS NOT NULL
             ORDER BY from_me DESC, timestamp DESC
             LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    /// Remembers a sticker the phone lists among its recent ones; a
    /// repeat keeps the later of the two "last used" times.
    pub fn upsert_phone_sticker(
        &self,
        hash: &str,
        raw: &[u8],
        last_used: i64,
        weight: f32,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO stickers (hash, raw, last_used, weight) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(hash) DO UPDATE SET
                 raw = excluded.raw,
                 last_used = MAX(stickers.last_used, excluded.last_used),
                 weight = excluded.weight",
            params![hash, raw, last_used, weight as f64],
        )?;
        Ok(())
    }

    /// The phone's recent stickers, most recently used first.
    pub fn phone_stickers(&self) -> Result<Vec<PhoneSticker>> {
        let mut statement = self.connection.prepare(
            "SELECT hash, raw, last_used, path FROM stickers
             ORDER BY last_used DESC, weight DESC
             LIMIT 120",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(PhoneSticker {
                hash: row.get(0)?,
                raw: row.get(1)?,
                last_used: row.get(2)?,
                path: row
                    .get::<_, Option<String>>(3)?
                    .map(std::path::PathBuf::from),
            })
        })?;
        rows.collect()
    }

    pub fn set_sticker_path(&self, hash: &str, path: &Path) -> Result<()> {
        self.connection.execute(
            "UPDATE stickers SET path = ?2 WHERE hash = ?1",
            params![hash, path.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Every message that still has its protobuf, for re-deriving what a
    /// newer version reads out of it.
    pub fn rows_with_raw(&self) -> Result<Vec<(String, String, Vec<u8>)>> {
        let mut statement = self
            .connection
            .prepare("SELECT chat, id, raw FROM messages WHERE raw IS NOT NULL")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    }

    /// Replaces what was derived from a message's protobuf, keeping what
    /// only this computer knows (the downloaded file).
    pub fn set_derived(
        &self,
        chat: &str,
        id: &str,
        content: &Content,
        mentions: &[crate::model::MentionRef],
        thumbnail: Option<&[u8]>,
        forwarded: bool,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE messages SET content = ?3, mentions = ?4, thumbnail = COALESCE(?5, thumbnail), forwarded = ?6
             WHERE chat = ?1 AND id = ?2",
            params![
                chat,
                id,
                serde_json::to_string(content).unwrap_or_default(),
                serde_json::to_string(mentions).unwrap_or_default(),
                thumbnail,
                forwarded
            ],
        )?;
        Ok(())
    }

    pub fn delete_message(&self, chat: &str, id: &str) -> Result<bool> {
        let deleted = self.connection.execute(
            "DELETE FROM messages WHERE chat = ?1 AND id = ?2",
            params![chat, id],
        )?;
        Ok(deleted > 0)
    }

    pub fn message(&self, chat: &str, id: &str) -> Result<Option<Message>> {
        let mut statement = self.connection.prepare(
            "SELECT sender, sender_name, from_me, timestamp, content, status, quoted, reactions, edited, thumbnail, mentions, forwarded, delivered_at, read_at
             FROM messages WHERE chat = ?1 AND id = ?2",
        )?;
        statement
            .query_row(params![chat, id], |row| {
                let content: String = row.get(4)?;
                let quoted: Option<String> = row.get(6)?;
                let reactions: String = row.get(7)?;
                let mentions: String = row.get(10)?;
                Ok(Message {
                    id: id.to_owned(),
                    chat: chat.to_owned(),
                    sender: row.get(0)?,
                    sender_name: row.get(1)?,
                    from_me: row.get(2)?,
                    timestamp: row.get(3)?,
                    content: serde_json::from_str(&content).unwrap_or(Content::Unsupported {
                        what: "unreadable".into(),
                    }),
                    status: status_from_rank(row.get(5)?),
                    delivered_at: row.get(12)?,
                    read_at: row.get(13)?,
                    quoted: quoted.and_then(|quoted| serde_json::from_str(&quoted).ok()),
                    reactions: serde_json::from_str(&reactions).unwrap_or_default(),
                    edited: row.get(8)?,
                    mentions: serde_json::from_str(&mentions).unwrap_or_default(),
                    forwarded: row.get(11)?,
                    thumbnail: row.get(9)?,
                })
            })
            .optional()
    }

    /// The earliest message of a chat, for asking the phone what came
    /// before it.
    pub fn oldest(&self, chat: &str) -> Result<Option<Message>> {
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM messages WHERE chat = ?1 ORDER BY timestamp ASC, rowid ASC LIMIT 1",
                params![chat],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.message(chat, &id),
            None => Ok(None),
        }
    }

    /// The protobuf a message arrived as, for fetching its attachment.
    pub fn raw(&self, chat: &str, id: &str) -> Result<Option<Vec<u8>>> {
        self.connection
            .query_row(
                "SELECT raw FROM messages WHERE chat = ?1 AND id = ?2",
                params![chat, id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// Moves a message's status forward; a receipt never moves it back,
    /// except to `Failed`. `at` is the receipt's time, kept as the moment
    /// the message was delivered or read; the first receipt of a stage
    /// wins.
    pub fn set_status(&self, chat: &str, id: &str, status: Delivery, at: i64) -> Result<bool> {
        let rank = status_rank(status);
        let changed = if status == Delivery::Failed {
            self.connection.execute(
                "UPDATE messages SET status = ?3 WHERE chat = ?1 AND id = ?2",
                params![chat, id, rank],
            )?
        } else if let Some(column) = stamp_column(status) {
            self.connection.execute(
                &format!(
                    "UPDATE messages SET status = ?3, {column} = COALESCE({column}, ?4)
                     WHERE chat = ?1 AND id = ?2 AND status < ?3"
                ),
                params![chat, id, rank, at],
            )?
        } else {
            self.connection.execute(
                "UPDATE messages SET status = ?3 WHERE chat = ?1 AND id = ?2 AND status < ?3",
                params![chat, id, rank],
            )?
        };
        Ok(changed > 0)
    }

    /// Every message of ours in a chat at or before `timestamp` that has not
    /// reached `status` yet, moved there. Returns the ids that changed.
    pub fn advance_statuses(
        &self,
        chat: &str,
        up_to: i64,
        status: Delivery,
        at: i64,
    ) -> Result<Vec<String>> {
        let rank = status_rank(status);
        let mut statement = self.connection.prepare(
            "SELECT id FROM messages WHERE chat = ?1 AND from_me = 1 AND timestamp <= ?2 AND status > 0 AND status < ?3",
        )?;
        let ids: Vec<String> = statement
            .query_map(params![chat, up_to, rank], |row| row.get(0))?
            .collect::<Result<_>>()?;
        if let Some(column) = stamp_column(status) {
            self.connection.execute(
                &format!(
                    "UPDATE messages SET status = ?3, {column} = COALESCE({column}, ?4)
                     WHERE chat = ?1 AND from_me = 1 AND timestamp <= ?2 AND status > 0 AND status < ?3"
                ),
                params![chat, up_to, rank, at],
            )?;
        } else {
            self.connection.execute(
                "UPDATE messages SET status = ?3 WHERE chat = ?1 AND from_me = 1 AND timestamp <= ?2 AND status > 0 AND status < ?3",
                params![chat, up_to, rank],
            )?;
        }
        Ok(ids)
    }

    pub fn set_content(
        &self,
        chat: &str,
        id: &str,
        content: &Content,
        edited: bool,
    ) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE messages SET content = ?3, edited = ?4 WHERE chat = ?1 AND id = ?2",
            params![
                chat,
                id,
                serde_json::to_string(content).unwrap_or_default(),
                edited
            ],
        )?;
        Ok(changed > 0)
    }

    /// Adds, replaces, or (with an empty emoji) removes a sender's reaction.
    pub fn set_reaction(
        &self,
        chat: &str,
        id: &str,
        sender: &str,
        from_me: bool,
        emoji: &str,
    ) -> Result<Option<Message>> {
        let Some(mut message) = self.message(chat, id)? else {
            return Ok(None);
        };
        message
            .reactions
            .retain(|reaction| reaction.sender != sender);
        if !emoji.is_empty() {
            message.reactions.push(crate::model::Reaction {
                sender: sender.to_owned(),
                from_me,
                emoji: emoji.to_owned(),
            });
        }
        self.connection.execute(
            "UPDATE messages SET reactions = ?3 WHERE chat = ?1 AND id = ?2",
            params![
                chat,
                id,
                serde_json::to_string(&message.reactions).unwrap_or_default()
            ],
        )?;
        Ok(Some(message))
    }

    pub fn upsert_contact(&self, contact: &Contact) -> Result<()> {
        self.connection.execute(
            "INSERT INTO contacts (id, full_name, push_name) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                full_name = COALESCE(excluded.full_name, full_name),
                push_name = COALESCE(excluded.push_name, push_name)",
            params![contact.id, contact.full_name, contact.push_name],
        )?;
        Ok(())
    }

    pub fn contacts(&self) -> Result<Vec<Contact>> {
        let mut statement = self
            .connection
            .prepare("SELECT id, full_name, push_name FROM contacts")?;
        let rows = statement.query_map([], |row| {
            Ok(Contact {
                id: row.get(0)?,
                full_name: row.get(1)?,
                push_name: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Forgets everything, for unlinking the device.
    pub fn clear(&self) -> Result<()> {
        self.connection.execute_batch(
            "DELETE FROM messages; DELETE FROM chats; DELETE FROM contacts; DELETE FROM meta; DELETE FROM lids;",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Content;

    fn message(chat: &str, id: &str, timestamp: i64, from_me: bool) -> Message {
        Message {
            id: id.into(),
            chat: chat.into(),
            sender: if from_me { "me@s.whatsapp.net" } else { chat }.into(),
            sender_name: None,
            from_me,
            timestamp,
            content: Content::text(format!("message {id}")),
            status: if from_me {
                Delivery::Pending
            } else {
                Delivery::None
            },
            delivered_at: None,
            read_at: None,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    #[test]
    fn search_finds_text_captions_and_file_names() {
        let archive = Archive::in_memory().expect("opens");
        archive
            .ensure_chat("1@s.whatsapp.net", "Ada")
            .expect("chat");
        let media = || crate::model::Media {
            mime: "application/pdf".into(),
            size: 1,
            width: None,
            height: None,
            path: None,
            state: crate::model::MediaState::Idle,
        };
        let mut plain = message("1@s.whatsapp.net", "m1", 10, false);
        plain.content = Content::text("The Difference Engine assembles");
        let mut caption = message("1@s.whatsapp.net", "m2", 20, true);
        caption.content = Content::Document {
            media: media(),
            file_name: "Notes on the Engine.pdf".into(),
            caption: Some("progress at 100% now".into()),
            pages: None,
        };
        let mut other = message("1@s.whatsapp.net", "m3", 30, false);
        other.content = Content::text("Nothing of note");
        for row in [&plain, &caption, &other] {
            archive.insert_message(row, None).expect("insert");
        }
        // Body and file name both match, newest first, whatever the case.
        let hits = archive.search_messages("ENGINE", 10).expect("search");
        let ids: Vec<&str> = hits.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, vec!["m2", "m1"]);
        // A LIKE wildcard in the query is a character, not a wildcard.
        let hits = archive.search_messages("100%", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "m2");
        assert!(
            archive
                .search_messages("100&", 10)
                .expect("search")
                .is_empty(),
            "the percent sign was matched literally"
        );
        assert!(
            archive
                .search_messages("zebra", 10)
                .expect("search")
                .is_empty()
        );
        // The limit caps the answer.
        let hits = archive.search_messages("e", 1).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "m3", "newest first");
    }

    #[test]
    fn migrations_add_columns_to_an_older_archive() {
        let connection = Connection::open_in_memory().expect("opens");
        connection
            .execute_batch(
                "CREATE TABLE chats (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
                    last_activity INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
                    archived INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0, muted_until INTEGER);
                 INSERT INTO chats (id, name, kind) VALUES ('1@s.whatsapp.net', 'A', 'direct');",
            )
            .expect("old schema");
        let archive = Archive::prepare(connection).expect("migrates");
        let chats = archive.chats().expect("chats");
        assert_eq!(chats.len(), 1);
        assert!(chats[0].participants.is_empty());
        assert!(!chats[0].read_only);
        let mut with_thumbnail = message("1@s.whatsapp.net", "m1", 1, false);
        with_thumbnail.thumbnail = Some(vec![1, 2, 3]);
        archive
            .insert_message(&with_thumbnail, None)
            .expect("insert");
        assert_eq!(
            archive
                .message("1@s.whatsapp.net", "m1")
                .expect("read")
                .expect("exists")
                .thumbnail,
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            archive
                .oldest("1@s.whatsapp.net")
                .expect("oldest")
                .map(|m| m.id),
            Some("m1".into())
        );
    }

    #[test]
    fn group_info_is_kept() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1-2@g.us";
        archive.ensure_chat(chat, "Group").expect("chat");
        archive
            .set_group_info(
                chat,
                Some("Rust Berlin"),
                &["a@s.whatsapp.net".into()],
                true,
            )
            .expect("info");
        let row = archive.chat(chat).expect("chat").expect("exists");
        assert_eq!(row.name, "Rust Berlin");
        assert_eq!(row.participants, vec!["a@s.whatsapp.net"]);
        assert!(row.read_only);
        archive
            .set_group_info(chat, None, &[], false)
            .expect("info");
        assert_eq!(
            archive.chat(chat).expect("chat").expect("exists").name,
            "Rust Berlin"
        );
    }

    #[test]
    fn chats_order_by_activity_and_carry_their_last_message() {
        let archive = Archive::in_memory().expect("opens");
        let a = "1@s.whatsapp.net";
        let b = "2@s.whatsapp.net";
        archive.ensure_chat(a, "A").expect("chat");
        archive.ensure_chat(b, "B").expect("chat");
        archive
            .insert_message(&message(a, "m1", 100, false), None)
            .expect("insert");
        archive
            .insert_message(&message(b, "m2", 200, true), None)
            .expect("insert");
        archive
            .insert_message(&message(a, "m3", 150, false), None)
            .expect("insert");
        let chats = archive.chats().expect("chats");
        assert_eq!(chats[0].id, b);
        assert_eq!(
            chats[0].last.as_ref().map(|last| last.summary.as_str()),
            Some("message m2")
        );
        assert_eq!(
            chats[0].last.as_ref().map(|last| last.status),
            Some(Delivery::Pending)
        );
        assert_eq!(chats[1].id, a);
        assert_eq!(chats[1].last_activity, 150);
        assert_eq!(
            chats[1].last.as_ref().map(|last| last.summary.as_str()),
            Some("message m3")
        );
    }

    #[test]
    fn statuses_only_move_forward() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        archive
            .insert_message(&message(chat, "m1", 100, true), None)
            .expect("insert");
        assert!(
            archive
                .set_status(chat, "m1", Delivery::Read, 500)
                .expect("status")
        );
        assert!(
            !archive
                .set_status(chat, "m1", Delivery::Delivered, 600)
                .expect("status")
        );
        let stored = archive.message(chat, "m1").expect("read").expect("exists");
        assert_eq!(stored.status, Delivery::Read);
        assert_eq!(stored.read_at, Some(500));
        assert_eq!(stored.delivered_at, None);
        // Reinserting the same message (a history sync replay) keeps Read.
        archive
            .insert_message(&message(chat, "m1", 100, true), None)
            .expect("insert");
        assert_eq!(
            archive
                .message(chat, "m1")
                .expect("read")
                .expect("exists")
                .status,
            Delivery::Read
        );
        assert!(
            archive
                .set_status(chat, "m1", Delivery::Failed, 700)
                .expect("status")
        );
    }

    #[test]
    fn a_read_receipt_covers_everything_before_it() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        for (id, timestamp) in [("m1", 100), ("m2", 200), ("m3", 300)] {
            archive
                .insert_message(&message(chat, id, timestamp, true), None)
                .expect("insert");
        }
        archive
            .insert_message(&message(chat, "theirs", 250, false), None)
            .expect("insert");
        let changed = archive
            .advance_statuses(chat, 200, Delivery::Read, 400)
            .expect("advance");
        assert_eq!(changed, vec!["m1", "m2"]);
        let messages = archive.messages(chat, None, 10).expect("messages");
        let statuses: Vec<Delivery> = messages.iter().map(|message| message.status).collect();
        assert_eq!(
            statuses,
            vec![
                Delivery::Read,
                Delivery::Read,
                Delivery::None,
                Delivery::Pending
            ]
        );
        assert_eq!(messages[0].read_at, Some(400));
    }

    #[test]
    fn paging_walks_backwards_in_time() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        for index in 0..10 {
            archive
                .insert_message(
                    &message(chat, &format!("m{index}"), 100 + index, false),
                    None,
                )
                .expect("insert");
        }
        let newest = archive.messages(chat, None, 3).expect("messages");
        assert_eq!(
            newest.iter().map(|m| m.timestamp).collect::<Vec<_>>(),
            vec![107, 108, 109]
        );
        let older = archive
            .messages(chat, Some((107, "m7")), 3)
            .expect("messages");
        assert_eq!(
            older.iter().map(|m| m.timestamp).collect::<Vec<_>>(),
            vec![104, 105, 106]
        );
    }

    #[test]
    fn paging_keeps_every_message_of_a_second() {
        // An album: five pictures in one second, paged three at a time.
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        archive
            .insert_message(&message(chat, "before", 99, false), None)
            .expect("insert");
        for index in 0..5 {
            archive
                .insert_message(&message(chat, &format!("a{index}"), 100, false), None)
                .expect("insert");
        }
        let first = archive.messages(chat, None, 3).expect("messages");
        assert_eq!(
            first.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a2", "a3", "a4"]
        );
        let oldest = &first[0];
        let second = archive
            .messages(chat, Some((oldest.timestamp, &oldest.id)), 3)
            .expect("messages");
        assert_eq!(
            second.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["before", "a0", "a1"],
            "the rest of the second comes next, not the message before it alone"
        );
        let range = archive
            .messages_range(chat, 100, (100, "a2"), 10)
            .expect("range");
        assert_eq!(
            range.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a0", "a1"]
        );
    }

    #[test]
    fn ranges_and_deletion() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        for index in 0..6 {
            archive
                .insert_message(
                    &message(chat, &format!("m{index}"), 100 + index, false),
                    None,
                )
                .expect("insert");
        }
        let range = archive
            .messages_range(chat, 102, (105, "m5"), 10)
            .expect("range");
        assert_eq!(
            range.iter().map(|m| m.timestamp).collect::<Vec<_>>(),
            vec![102, 103, 104]
        );
        assert!(archive.delete_message(chat, "m3").expect("delete"));
        assert!(!archive.delete_message(chat, "m3").expect("delete"));
        assert!(archive.message(chat, "m3").expect("read").is_none());
    }

    #[test]
    fn reactions_replace_per_sender() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        archive
            .insert_message(&message(chat, "m1", 100, false), None)
            .expect("insert");
        archive
            .set_reaction(chat, "m1", chat, false, "👍")
            .expect("react");
        let updated = archive
            .set_reaction(chat, "m1", chat, false, "❤️")
            .expect("react")
            .expect("exists");
        assert_eq!(updated.reactions.len(), 1);
        assert_eq!(updated.reactions[0].emoji, "❤️");
        let removed = archive
            .set_reaction(chat, "m1", chat, false, "")
            .expect("react")
            .expect("exists");
        assert!(removed.reactions.is_empty());
    }

    #[test]
    fn unread_counts_and_incoming_ids_track_the_other_side() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        archive
            .insert_message(&message(chat, "m1", 100, false), None)
            .expect("insert");
        archive.bump_unread(chat).expect("bump");
        archive
            .insert_message(&message(chat, "mine", 150, true), None)
            .expect("insert");
        archive
            .insert_message(&message(chat, "m2", 200, false), None)
            .expect("insert");
        archive.bump_unread(chat).expect("bump");
        assert_eq!(archive.chat(chat).expect("chat").expect("exists").unread, 2);
        let ids: Vec<String> = archive
            .unread_incoming(chat, 2)
            .expect("ids")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec!["m2", "m1"]);
        archive.mark_read(chat).expect("read");
        assert_eq!(archive.chat(chat).expect("chat").expect("exists").unread, 0);
    }

    #[test]
    fn media_paths_are_written_into_the_content() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        let mut picture = message(chat, "p1", 100, false);
        picture.content = Content::Image {
            caption: None,
            media: crate::model::Media {
                mime: "image/jpeg".into(),
                size: 10,
                width: None,
                height: None,
                path: None,
                state: Default::default(),
            },
        };
        archive.insert_message(&picture, None).expect("insert");
        let updated = archive
            .set_media_path(chat, "p1", Path::new("/tmp/p1.jpg"))
            .expect("set")
            .expect("exists");
        assert_eq!(
            updated.content.media().and_then(|media| media.path.clone()),
            Some(std::path::PathBuf::from("/tmp/p1.jpg"))
        );
        let reread = archive.message(chat, "p1").expect("read").expect("exists");
        assert_eq!(reread.content, updated.content);
    }

    #[test]
    fn raw_bytes_survive_a_replay_without_them() {
        let archive = Archive::in_memory().expect("opens");
        let chat = "1@s.whatsapp.net";
        archive.ensure_chat(chat, "A").expect("chat");
        archive
            .insert_message(&message(chat, "m1", 100, false), Some(&[1, 2, 3]))
            .expect("insert");
        archive
            .insert_message(&message(chat, "m1", 100, false), None)
            .expect("insert");
        assert_eq!(archive.raw(chat, "m1").expect("raw"), Some(vec![1, 2, 3]));
    }
}

#[cfg(test)]
mod sticker_tests {
    use super::*;
    use crate::model::{Content, Delivery, Media, MediaState};

    fn sticker(chat: &str, id: &str, timestamp: i64, path: Option<&str>) -> Message {
        Message {
            id: id.into(),
            chat: chat.into(),
            sender: chat.into(),
            sender_name: None,
            from_me: false,
            timestamp,
            content: Content::Sticker {
                media: Media {
                    mime: "image/webp".into(),
                    size: 10,
                    width: Some(512),
                    height: Some(512),
                    path: path.map(std::path::PathBuf::from),
                    state: MediaState::Idle,
                },
                animated: false,
            },
            status: Delivery::None,
            delivered_at: None,
            read_at: None,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    #[test]
    fn phone_stickers_keep_the_latest_use_and_their_file() {
        let archive = Archive::in_memory().expect("opens");
        archive
            .upsert_phone_sticker("aa", b"one", 100, 0.5)
            .expect("stored");
        archive
            .upsert_phone_sticker("bb", b"two", 300, 0.1)
            .expect("stored");
        // A repeat with an older "last used" does not move it back.
        archive
            .upsert_phone_sticker("aa", b"one", 50, 0.9)
            .expect("stored");
        let list = archive.phone_stickers().expect("lists");
        assert_eq!(
            list.iter().map(|s| s.hash.as_str()).collect::<Vec<_>>(),
            ["bb", "aa"]
        );
        assert_eq!(list[1].last_used, 100);
        assert!(list.iter().all(|s| s.path.is_none()));
        archive
            .set_sticker_path("aa", Path::new("/tmp/aa.webp"))
            .expect("filed");
        let list = archive.phone_stickers().expect("lists");
        assert_eq!(list[1].path.as_deref(), Some(Path::new("/tmp/aa.webp")));
    }

    #[test]
    fn unfetched_stickers_are_listed_for_the_picker_and_fetched_ones_are_not() {
        let archive = Archive::in_memory().expect("opens");
        archive.ensure_chat("a@s.whatsapp.net", "A").expect("chat");
        archive
            .insert_message(&sticker("a@s.whatsapp.net", "s1", 10, None), Some(b"raw"))
            .expect("inserted");
        archive
            .insert_message(
                &sticker("a@s.whatsapp.net", "s2", 20, Some("/nowhere/s2.webp")),
                Some(b"raw"),
            )
            .expect("inserted");
        let missing = archive.stickers_without_file(10).expect("lists");
        assert_eq!(
            missing,
            vec![("a@s.whatsapp.net".to_owned(), "s1".to_owned())]
        );
        // A file that is gone from disk is not offered.
        assert!(archive.recent_stickers(10).expect("lists").is_empty());
    }
}

#[cfg(test)]
mod media_path_tests {
    use super::*;
    use crate::model::{Content, Delivery, Media, MediaState};

    fn picture(id: &str) -> Message {
        Message {
            id: id.into(),
            chat: "a@s.whatsapp.net".into(),
            sender: "a@s.whatsapp.net".into(),
            sender_name: None,
            from_me: false,
            timestamp: 1,
            content: Content::Image {
                media: Media {
                    mime: "image/jpeg".into(),
                    size: 10,
                    width: None,
                    height: None,
                    path: None,
                    state: MediaState::Idle,
                },
                caption: None,
            },
            status: Delivery::None,
            delivered_at: None,
            read_at: None,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    #[test]
    fn attachment_paths_can_be_listed_moved_and_forgotten() {
        let archive = Archive::in_memory().expect("opens");
        archive.ensure_chat("a@s.whatsapp.net", "A").expect("chat");
        archive
            .insert_message(&picture("p1"), None)
            .expect("inserted");
        assert!(archive.media_paths().expect("lists").is_empty());
        archive
            .set_media_path("a@s.whatsapp.net", "p1", Path::new("/old/media/p1.jpg"))
            .expect("filed");
        let listed = archive.media_paths().expect("lists");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].2, Path::new("/old/media/p1.jpg"));
        archive
            .clear_media_path("a@s.whatsapp.net", "p1")
            .expect("cleared");
        assert!(archive.media_paths().expect("lists").is_empty());
    }
}
