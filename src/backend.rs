//! The bridge between the interface thread and everything asynchronous.
//!
//! egui runs on the main thread and must never block. A dedicated tokio
//! runtime hosts the WhatsApp connection, the message archive, and media
//! downloads; the two sides talk through channels. Every event wakes the
//! interface with `request_repaint`, so the app stays event-driven and idle
//! when nothing is happening.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::model::{Chat, ChatId, Contact, Gif, Message};
use crate::paths::AppDirs;

mod worker;

/// Where the link to the phone stands.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkStatus {
    Starting,
    /// Waiting for the phone to scan the code, or to accept the pair code
    /// if one was asked for.
    Unlinked {
        qr: Option<String>,
        pair_code: Option<String>,
        pairing_phone: Option<String>,
    },
    Connecting,
    Connected,
    /// The connection dropped; the library reconnects on its own.
    Disconnected {
        reason: String,
    },
    /// The phone unlinked this device.
    LoggedOut,
    Failed(String),
}

impl LinkStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// Where a page of messages ends: the time and id of the oldest message
/// loaded, so the next page starts right after it even when several
/// messages share that second.
pub type PageKey = (i64, String);

#[derive(Clone, Debug)]
pub enum Command {
    SendText {
        chat: ChatId,
        text: String,
        quoting: Option<String>,
    },
    /// We are, or stopped, typing in a chat.
    Composing {
        chat: ChatId,
        composing: bool,
    },
    /// The chat is on screen: clear its unread count, and send read
    /// receipts for what was unread when `receipts` is on.
    MarkRead {
        chat: ChatId,
        receipts: bool,
    },
    /// Messages of a chat from the archive, older than `before` when given.
    LoadChat {
        chat: ChatId,
        before: Option<PageKey>,
    },
    /// Ask the phone for what came before the archive's earliest message.
    FetchOlder(ChatId),
    Download {
        chat: ChatId,
        message: String,
    },
    /// A profile picture; `full` for the large one an info dialog shows.
    FetchAvatar {
        id: String,
        full: bool,
    },
    /// Every archived message of a chat from `id` up to what is loaded,
    /// so a quoted message can be scrolled to.
    LoadUntil {
        chat: ChatId,
        id: String,
        before: PageKey,
    },
    /// Internal: the phone could not be asked for older messages.
    OlderFailed {
        chat: ChatId,
        error: String,
    },
    /// Internal: a group's metadata could not be fetched; it will be asked
    /// for again.
    GroupInfoFailed {
        chat: ChatId,
        /// The server's refusal is final (not a member any more, say):
        /// asking again will not change it.
        permanent: bool,
    },
    EditText {
        chat: ChatId,
        id: String,
        text: String,
    },
    Revoke {
        chat: ChatId,
        id: String,
    },
    DeleteLocal {
        chat: ChatId,
        id: String,
    },
    /// Open the desktop's file picker and send what is chosen.
    PickFiles(ChatId),
    /// Files to send; the caption goes with the first.
    SendFiles {
        chat: ChatId,
        paths: Vec<PathBuf>,
        caption: Option<String>,
    },
    /// A picture off the clipboard, as straight RGBA.
    SendImage {
        chat: ChatId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        caption: Option<String>,
    },
    /// Mute a chat until the given time (seconds), `Some(0)` for good,
    /// `None` to unmute; told to the phone as well.
    SetMuted(ChatId, Option<i64>),
    /// A recorded voice message: mono samples at 48 kHz, normalized,
    /// encoded, and sent as push-to-talk, quoting a message if replying.
    SendVoice {
        chat: ChatId,
        samples: Vec<f32>,
        quoting: Option<String>,
    },
    /// Tell the sender their voice message was listened to.
    MarkPlayed {
        chat: ChatId,
        message: String,
        sender: String,
    },
    /// A sticker file (WebP) to send.
    SendSticker {
        chat: ChatId,
        path: PathBuf,
    },
    /// Fetch a GIF from the web and send it as WhatsApp does, a short
    /// looping video.
    SendGif {
        chat: ChatId,
        gif: Gif,
    },
    /// Ask GIPHY; an empty query lists what is trending.
    SearchGifs {
        query: String,
        key: String,
    },
    /// The stickers seen lately, for the picker.
    RecentStickers,
    React {
        chat: ChatId,
        message: String,
        emoji: String,
    },
    SetArchived(ChatId, bool),
    SetPinned(ChatId, bool),
    PairWithPhone(String),
    /// Forget this device on the phone and locally.
    Unlink,
    Reconnect,
    Shutdown,
    /// Internal: a send finished.
    Sent {
        chat: ChatId,
        id: String,
        error: Option<String>,
    },
    /// Internal: an attachment landed on disk, or did not.
    Downloaded {
        chat: ChatId,
        id: String,
        result: Result<PathBuf, String>,
    },
    /// Internal: one of the phone's recent stickers landed on disk, or did
    /// not.
    StickerFetched {
        hash: String,
        result: Result<PathBuf, String>,
    },
    /// Internal: a profile picture was looked up.
    AvatarFetched {
        id: String,
        full: bool,
        path: Option<PathBuf>,
    },
    /// Internal: a picture lookup failed and should be tried again later.
    AvatarFailed {
        id: String,
        full: bool,
    },
    /// Internal: our own "about" text.
    MeInfo {
        about: Option<String>,
    },
    /// Internal: GIPHY answered.
    GifResults {
        query: String,
        results: Result<Vec<Gif>, String>,
    },
    /// Internal: the file picker closed.
    Picked {
        chat: ChatId,
        paths: Vec<PathBuf>,
    },
    /// Internal: an attachment is uploaded and its message built; file it
    /// and send it.
    Outbound {
        chat: ChatId,
        row: Box<Message>,
        raw: Vec<u8>,
    },
    /// Internal: what WhatsApp knows about a group.
    GroupInfo {
        chat: ChatId,
        name: Option<String>,
        participants: Vec<String>,
        read_only: bool,
    },
    /// Internal: pairing by phone number produced a code, or failed.
    PairCode {
        result: Result<String, String>,
    },
}

#[derive(Debug)]
pub enum Event {
    Link(LinkStatus),
    /// Who we are, once known.
    Me {
        id: String,
        name: Option<String>,
        about: Option<String>,
    },
    /// The whole chat list, newest first.
    Chats(Vec<Chat>),
    ChatUpdated(Box<Chat>),
    /// Messages of a chat, oldest first. `older` means they go before what
    /// is shown; otherwise they are appended. `complete` says the archive
    /// has nothing earlier.
    Messages {
        chat: ChatId,
        messages: Vec<Message>,
        older: bool,
        complete: bool,
    },
    MessageUpdated(Box<Message>),
    /// Files chosen in the picker, for the composer to stage.
    Picked {
        chat: ChatId,
        paths: Vec<PathBuf>,
    },
    /// A message just arrived from someone else, live (not from history),
    /// for the desktop notification.
    Incoming {
        chat: ChatId,
        message: Box<Message>,
    },
    Contacts(Vec<Contact>),
    Typing {
        chat: ChatId,
        sender: String,
        composing: bool,
    },
    Presence {
        id: String,
        online: bool,
        last_seen: Option<i64>,
    },
    Avatar {
        id: String,
        full: bool,
        path: Option<PathBuf>,
    },
    MessageDeleted {
        chat: ChatId,
        id: String,
    },
    /// GIFs for a query, or why there are none.
    Gifs {
        query: String,
        results: Result<Vec<Gif>, String>,
    },
    /// Sticker files seen lately, newest first.
    Stickers(Vec<PathBuf>),
    Media {
        chat: ChatId,
        message: String,
        result: Result<PathBuf, String>,
    },
    /// History is being replayed after linking.
    Syncing(bool),
    /// How far the replay has come, when WhatsApp says.
    SyncProgress(u32),
    /// The phone answered (or did not) a request for older messages;
    /// `more` says whether asking again could bring more.
    OlderFetched {
        chat: ChatId,
        more: bool,
    },
    Error(String),
}

/// Wakes the window, if one exists, from any thread.
#[derive(Clone, Default)]
pub struct Waker(Arc<std::sync::Mutex<Option<egui::Context>>>);

impl Waker {
    pub fn attach(&self, ctx: &egui::Context) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = Some(ctx.clone());
    }

    pub fn detach(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    pub fn wake(&self) {
        if let Some(ctx) = self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            ctx.request_repaint();
        }
    }

    /// A repaint a little later, for something that moves on its own.
    pub fn wake_after(&self, delay: std::time::Duration) {
        if let Some(ctx) = self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            ctx.request_repaint_after(delay);
        }
    }
}

/// The interface's handle to the runtime.
pub struct Backend {
    commands: mpsc::UnboundedSender<Command>,
    events: std::sync::mpsc::Receiver<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
    offline: bool,
}

impl Backend {
    pub fn spawn(dirs: AppDirs, waker: Waker) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fastsapp-runtime")
            .enable_all()
            .build()
            .expect("unable to start the async runtime");
        let worker_commands = command_tx.clone();
        let thread = std::thread::Builder::new()
            .name("fastsapp-backend".to_string())
            .spawn(move || {
                runtime.block_on(async move {
                    worker::run(dirs, event_tx, worker_commands, command_rx, waker).await;
                });
                runtime.shutdown_timeout(Duration::from_secs(3));
            })
            .expect("unable to start the backend thread");

        Self {
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
            offline: false,
        }
    }

    /// A backend that never connects, for the demo mode and headless tests.
    /// Events can still be fed to the app through the returned sender.
    pub fn detached() -> (Self, std::sync::mpsc::Sender<Event>) {
        let (command_tx, _command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        (
            Self {
                commands: command_tx,
                events: event_rx,
                thread: None,
                offline: true,
            },
            event_tx,
        )
    }

    /// Stops commands from leaving the process; shutdown still works.
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    pub fn is_offline(&self) -> bool {
        self.offline
    }

    pub fn send(&self, command: Command) {
        if self.offline && !matches!(command, Command::Shutdown) {
            return;
        }
        let _ = self.commands.send(command);
    }

    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    pub fn shutdown(&mut self) {
        self.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
