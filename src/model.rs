//! What the interface shows: chats, messages, and the actions views emit.
//!
//! These are the app's own types, translated from the protocol's in
//! `backend.rs`, so a view never touches a protobuf and the archive on disk
//! has a stable shape.

use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// A chat is keyed by its JID as text: `<phone>@s.whatsapp.net` for a
/// person, `<id>@g.us` for a group, `<id>@lid` for a person behind a
/// privacy-preserving id.
pub type ChatId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    Direct,
    Group,
    /// A newsletter channel or a broadcast list; read-only here.
    Broadcast,
}

impl ChatKind {
    pub fn from_id(id: &str) -> Self {
        match id.rsplit('@').next() {
            Some("g.us") => Self::Group,
            Some("newsletter") | Some("broadcast") => Self::Broadcast,
            _ => Self::Direct,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chat {
    pub id: ChatId,
    /// The best name known: the address book's, then the push name, then
    /// the phone number.
    pub name: String,
    pub kind: ChatKind,
    /// Unix seconds of the newest message, for ordering.
    pub last_activity: i64,
    pub unread: u32,
    pub archived: bool,
    pub pinned: bool,
    /// Unix seconds until which notifications are off; `Some(0)` is forever.
    pub muted_until: Option<i64>,
    /// The newest message, as the chat list shows it.
    pub last: Option<LastMessage>,
    /// Members of a group, as canonical ids; empty until WhatsApp answers.
    pub participants: Vec<String>,
    /// An announcement group where only admins post, and we are not one.
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LastMessage {
    pub from_me: bool,
    pub sender: String,
    /// Who sent it, in a group.
    pub sender_name: Option<String>,
    pub summary: String,
    pub status: Delivery,
}

impl Chat {
    pub fn new(id: ChatId, name: String) -> Self {
        let kind = ChatKind::from_id(&id);
        Self {
            id,
            name,
            kind,
            last_activity: 0,
            unread: 0,
            archived: false,
            pinned: false,
            muted_until: None,
            last: None,
            participants: Vec::new(),
            read_only: false,
        }
    }

    pub fn is_group(&self) -> bool {
        self.kind == ChatKind::Group
    }

    pub fn muted(&self, now: i64) -> bool {
        matches!(self.muted_until, Some(0)) || self.muted_until.is_some_and(|until| until > now)
    }

    /// The phone number for a direct chat, as digits.
    pub fn phone(&self) -> Option<&str> {
        phone_of(&self.id)
    }
}

/// The digits of a `<phone>@s.whatsapp.net` id.
pub fn phone_of(id: &str) -> Option<&str> {
    let (user, server) = id.split_once('@')?;
    (server == "s.whatsapp.net" && user.chars().all(|c| c.is_ascii_digit())).then_some(user)
}

/// How far our own message has travelled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Not ours; receipts do not apply.
    #[default]
    None,
    /// Handed to the backend, not yet acknowledged by the server.
    Pending,
    Sent,
    Delivered,
    Read,
    Played,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// WhatsApp's id; unique within a chat.
    pub id: String,
    pub chat: ChatId,
    /// The sender's JID as text; ours when `from_me`.
    pub sender: String,
    /// The sender's push name when the message arrived, for groups.
    pub sender_name: Option<String>,
    pub from_me: bool,
    /// Unix seconds.
    pub timestamp: i64,
    pub content: Content,
    pub status: Delivery,
    pub quoted: Option<Quoted>,
    pub reactions: Vec<Reaction>,
    pub edited: bool,
    /// People named with `@` in the text or caption.
    #[serde(default)]
    pub mentions: Vec<MentionRef>,
    /// Passed on from another chat.
    #[serde(default)]
    pub forwarded: bool,
    /// The small picture WhatsApp sends ahead of an attachment or with a
    /// link preview, as JPEG bytes.
    #[serde(default)]
    pub thumbnail: Option<Vec<u8>>,
}

/// Someone named in a message: the digits after the `@` as WhatsApp wrote
/// them, and the canonical id they stand for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MentionRef {
    pub user: String,
    pub id: String,
}

/// What WhatsApp found behind a link when the message was sent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

impl Message {
    /// One line describing the message, for chat rows and quotes.
    pub fn summary(&self) -> String {
        self.content.summary()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quoted {
    pub id: String,
    pub sender: String,
    pub sender_name: Option<String>,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub sender: String,
    pub from_me: bool,
    pub emoji: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    Text {
        text: String,
        #[serde(default)]
        preview: Option<LinkPreview>,
    },
    Image {
        caption: Option<String>,
        media: Media,
    },
    Video {
        caption: Option<String>,
        media: Media,
        seconds: Option<u32>,
        gif: bool,
    },
    Audio {
        media: Media,
        seconds: Option<u32>,
        voice_note: bool,
    },
    Document {
        media: Media,
        file_name: String,
        caption: Option<String>,
        pages: Option<u32>,
    },
    Sticker {
        media: Media,
        animated: bool,
    },
    Location {
        latitude: f64,
        longitude: f64,
        name: Option<String>,
        address: Option<String>,
    },
    Contact {
        display_name: String,
        vcard: String,
    },
    Poll {
        question: String,
        options: Vec<String>,
    },
    /// "This message was deleted."
    Revoked,
    /// Something this client does not render; `what` names the protobuf
    /// field so the user knows what they are missing.
    Unsupported {
        what: String,
    },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            preview: None,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Text { text, .. } => text.lines().next().unwrap_or_default().to_owned(),
            Self::Image { caption, .. } => with_caption("Photo", caption),
            Self::Video { caption, gif, .. } => {
                with_caption(if *gif { "GIF" } else { "Video" }, caption)
            }
            Self::Audio {
                voice_note,
                seconds,
                ..
            } => {
                let label = if *voice_note {
                    "Voice message"
                } else {
                    "Audio"
                };
                match seconds {
                    Some(seconds) => format!("{label} ({})", crate::util::duration(*seconds)),
                    None => label.to_owned(),
                }
            }
            Self::Document { file_name, .. } => format!("Document: {file_name}"),
            Self::Sticker { .. } => "Sticker".to_owned(),
            Self::Location { name, .. } => match name {
                Some(name) => format!("Location: {name}"),
                None => "Location".to_owned(),
            },
            Self::Contact { display_name, .. } => format!("Contact: {display_name}"),
            Self::Poll { question, .. } => format!("Poll: {question}"),
            Self::Revoked => "This message was deleted".to_owned(),
            Self::Unsupported { what } => format!("Unsupported message ({what})"),
        }
    }

    pub fn media(&self) -> Option<&Media> {
        match self {
            Self::Image { media, .. }
            | Self::Video { media, .. }
            | Self::Audio { media, .. }
            | Self::Document { media, .. }
            | Self::Sticker { media, .. } => Some(media),
            _ => None,
        }
    }

    pub fn media_mut(&mut self) -> Option<&mut Media> {
        match self {
            Self::Image { media, .. }
            | Self::Video { media, .. }
            | Self::Audio { media, .. }
            | Self::Document { media, .. }
            | Self::Sticker { media, .. } => Some(media),
            _ => None,
        }
    }
}

fn with_caption(label: &str, caption: &Option<String>) -> String {
    match caption
        .as_deref()
        .and_then(|caption| caption.lines().next())
    {
        Some(caption) if !caption.is_empty() => format!("{label}: {caption}"),
        _ => label.to_owned(),
    }
}

/// An attachment: what is known before it is fetched, and the file once it
/// has been. The keys to fetch it stay in the raw message the archive
/// keeps, never here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub mime: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The decrypted file, once downloaded.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Live download state; not persisted.
    #[serde(skip)]
    pub state: MediaState,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum MediaState {
    #[default]
    Idle,
    Downloading,
    Failed(String),
}

/// A person the account knows: the address book name arrives through app
/// state sync, the push name with each message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contact {
    pub id: String,
    pub full_name: Option<String>,
    pub push_name: Option<String>,
}

impl Contact {
    pub fn display_name(&self) -> Option<&str> {
        self.full_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or(self.push_name.as_deref().filter(|name| !name.is_empty()))
    }

    /// The name as WhatsApp shows it: the address book's as is, a name the
    /// person chose for themselves with a tilde in front.
    pub fn label(&self) -> Option<String> {
        if let Some(name) = self.full_name.as_deref().filter(|name| !name.is_empty()) {
            return Some(name.to_owned());
        }
        self.push_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(|name| format!("~{name}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Chats,
    Settings,
}

/// The tabs of the picker above the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerTab {
    Emoji,
    Gifs,
    Stickers,
}

/// A GIF found through GIPHY.
#[derive(Clone, Debug, PartialEq)]
pub struct Gif {
    pub id: String,
    /// A still frame on disk, once fetched.
    pub still: Option<PathBuf>,
    pub mp4: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Shortcuts,
    About,
    ConfirmUnlink,
    /// Link with a phone number instead of a QR code; holds the number typed.
    PairWithPhone,
    ChatInfo(ChatId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub created: Instant,
}

/// Everything a view can ask the app to do. Views push these while drawing;
/// the app applies them after the frame, so no view mutates state it is
/// borrowing.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open(Page),
    OpenChat(ChatId),
    CloseChat,
    SendText {
        chat: ChatId,
        text: String,
        /// The message being replied to.
        quoting: Option<String>,
    },
    /// Say whether we are typing in a chat.
    Composing {
        chat: ChatId,
        composing: bool,
    },
    MarkRead(ChatId),
    LoadOlder(ChatId),
    /// Ask the phone for messages older than what the archive has.
    FetchOlder(ChatId),
    Download {
        chat: ChatId,
        message: String,
    },
    OpenFile(PathBuf),
    OpenUrl(String),
    CopyText(String),
    /// Start composing a reply to a message in the open chat.
    Reply(String),
    CancelReply,
    /// Put one of our own messages back in the composer to change it.
    Edit(String),
    CancelEdit,
    /// Take back a message of ours from everyone.
    DeleteForEveryone(String),
    /// Forget a message on this computer only.
    DeleteForMe(String),
    /// Choose files to send to the open chat.
    Attach,
    SendFiles(Vec<PathBuf>),
    /// A picture from the clipboard, as straight RGBA.
    PasteImage {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    /// Open the picker on a tab, or close it if that tab is showing.
    TogglePicker(PickerTab),
    ClosePicker,
    /// Put an emoji into the composer where the cursor is.
    InsertEmoji(String),
    SendSticker(PathBuf),
    /// Look GIFs up; an empty query lists what is trending.
    SearchGifs(String),
    SendGif(Gif),
    React {
        chat: ChatId,
        message: String,
        emoji: String,
    },
    SetArchived(ChatId, bool),
    SetPinned(ChatId, bool),
    ShowDialog(Dialog),
    CloseDialog,
    ToggleSidebar,
    FocusSearch,
    FocusComposer,
    ScrollToBottom,
    /// Bring a message of the open chat into view.
    ScrollTo(String),
    /// The chat list's search text changed.
    Search(String),
    SettingsChanged,
    ZoomBy(f32),
    ResetZoom,
    /// Ask WhatsApp for a pairing code for this phone number.
    PairWithPhone(String),
    /// Forget this device on the phone and locally.
    Unlink,
    Reconnect,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media() -> Media {
        Media {
            mime: "image/jpeg".into(),
            size: 1,
            width: None,
            height: None,
            path: None,
            state: MediaState::Idle,
        }
    }

    #[test]
    fn kinds_come_from_the_server_part() {
        assert_eq!(ChatKind::from_id("1@s.whatsapp.net"), ChatKind::Direct);
        assert_eq!(ChatKind::from_id("1@lid"), ChatKind::Direct);
        assert_eq!(ChatKind::from_id("1-2@g.us"), ChatKind::Group);
        assert_eq!(ChatKind::from_id("1@newsletter"), ChatKind::Broadcast);
    }

    #[test]
    fn summaries_read_like_whatsapp() {
        assert_eq!(Content::text("hi\nthere").summary(), "hi");
        assert_eq!(
            Content::Image {
                caption: Some("look".into()),
                media: media()
            }
            .summary(),
            "Photo: look"
        );
        assert_eq!(
            Content::Image {
                caption: None,
                media: media()
            }
            .summary(),
            "Photo"
        );
        assert_eq!(
            Content::Audio {
                media: media(),
                seconds: Some(65),
                voice_note: true
            }
            .summary(),
            "Voice message (1:05)"
        );
    }

    #[test]
    fn phones_only_come_from_phone_ids() {
        assert_eq!(
            phone_of("393331234567@s.whatsapp.net"),
            Some("393331234567")
        );
        assert_eq!(phone_of("12345@lid"), None);
        assert_eq!(phone_of("1-2@g.us"), None);
    }

    #[test]
    fn labels_mark_names_people_chose_themselves() {
        let saved = Contact {
            id: "1".into(),
            full_name: Some("Ada".into()),
            push_name: Some("ada l".into()),
        };
        assert_eq!(saved.label().as_deref(), Some("Ada"));
        let stranger = Contact {
            id: "2".into(),
            full_name: None,
            push_name: Some("Bob".into()),
        };
        assert_eq!(stranger.label().as_deref(), Some("~Bob"));
        assert_eq!(Contact::default().label(), None);
    }

    #[test]
    fn old_text_content_still_parses() {
        let old: Content = serde_json::from_str(r#"{"kind":"text","text":"hi"}"#).expect("parses");
        assert_eq!(old, Content::text("hi"));
    }

    #[test]
    fn content_survives_json() {
        let content = Content::Document {
            media: media(),
            file_name: "a.pdf".into(),
            caption: None,
            pages: Some(3),
        };
        let json = serde_json::to_string(&content).expect("serializes");
        let back: Content = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, content);
    }
}
