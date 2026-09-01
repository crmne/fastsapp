//! Channel bridge between the UI and asynchronous runtime.
//!
//! A dedicated tokio runtime owns the WhatsApp connection, archive, and media
//! work. Commands and events cross channels, and events wake the UI.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::model::{Chat, ChatId, Contact, Gif, GifError, Message, StickerPack};
use crate::paths::AppDirs;

// Re-exported so the picker can detect pasted Signal pack links.
pub(crate) mod sticker_import;
mod worker;

/// Phone-link state.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkStatus {
    Starting,
    /// Waiting for QR scanning or pairing-code acceptance.
    Unlinked {
        qr: Option<String>,
        pair_code: Option<String>,
        pairing_phone: Option<String>,
    },
    Connecting,
    Connected,
    /// Connection dropped and automatic reconnection is active.
    Disconnected {
        reason: String,
    },
    /// Device unlinked by the phone.
    LoggedOut,
    Failed(String),
}

impl LinkStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// Oldest loaded message timestamp and id used as a page boundary.
pub type PageKey = (i64, String);

#[derive(Clone, Debug)]
pub enum Command {
    SendText {
        chat: ChatId,
        text: String,
        quoting: Option<String>,
    },
    /// Updates our typing state in a chat.
    Composing {
        chat: ChatId,
        composing: bool,
    },
    /// Marks a visible chat read and optionally sends receipts.
    MarkRead {
        chat: ChatId,
        receipts: bool,
    },
    /// Loads archived chat messages before an optional boundary.
    LoadChat {
        chat: ChatId,
        before: Option<PageKey>,
    },
    /// Requests messages before the archive's earliest message.
    FetchOlder(ChatId),
    Download {
        chat: ChatId,
        message: String,
    },
    /// Requests a profile picture; `full` selects the info-dialog size.
    FetchAvatar {
        id: String,
        full: bool,
    },
    /// Loads archived messages from `id` through the current page.
    LoadUntil {
        chat: ChatId,
        id: String,
        before: PageKey,
    },
    /// Searches visible archived message text.
    SearchMessages {
        query: String,
    },
    /// Creates an archive chat before its first message is sent.
    EnsureChat {
        chat: ChatId,
        name: String,
    },
    /// Internal result for a failed phone-history request.
    OlderFailed {
        chat: ChatId,
        error: String,
    },
    /// Internal group-metadata failure.
    GroupInfoFailed {
        chat: ChatId,
        /// Whether the server refusal is permanent.
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
    /// Selects and sends files with the desktop picker.
    PickFiles(ChatId),
    /// Sends files with the caption on the first.
    SendFiles {
        chat: ChatId,
        paths: Vec<PathBuf>,
        caption: Option<String>,
    },
    /// Sends a clipboard image as straight-alpha RGBA.
    SendImage {
        chat: ChatId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        caption: Option<String>,
    },
    /// Syncs chat mute state. `Some(0)` is indefinite and `None` unmutes.
    SetMuted(ChatId, Option<i64>),
    /// Normalizes, encodes, and sends mono 48 kHz push-to-talk audio.
    SendVoice {
        chat: ChatId,
        samples: Vec<f32>,
        quoting: Option<String>,
    },
    /// Sends a played receipt for a voice message.
    MarkPlayed {
        chat: ChatId,
        message: String,
        sender: String,
    },
    /// Sends a WebP sticker.
    SendSticker {
        chat: ChatId,
        path: PathBuf,
    },
    /// Saves a sticker file.
    SaveSticker {
        path: PathBuf,
    },
    /// Removes a saved sticker.
    ForgetSticker {
        path: PathBuf,
    },
    /// Imports a pack from a signal.art link.
    ImportStickerUrl {
        url: String,
    },
    /// Selects and imports a .wastickers or zip archive.
    PickStickerArchive,
    /// Deletes an imported pack directory.
    DeleteStickerPack {
        dir: PathBuf,
    },
    /// Internal pack-import result. An empty error means the picker was canceled.
    StickerPackImported {
        result: Result<String, String>,
    },
    /// Saves a name through contact sync. `first_name` is the short display
    /// name; `to_phone` also adds it to the phone's address book.
    SaveContact {
        id: String,
        full_name: String,
        first_name: Option<String>,
        to_phone: bool,
    },
    /// Internal contact-save result.
    ContactSaved {
        id: String,
        name: String,
        error: Option<String>,
    },
    /// Checks a number, optionally saves it, and opens its chat.
    NewContact {
        phone: String,
        full_name: Option<String>,
        first_name: Option<String>,
        to_phone: bool,
    },
    /// Internal number-lookup result.
    ContactChecked {
        phone: String,
        full_name: Option<String>,
        first_name: Option<String>,
        to_phone: bool,
        registered: bool,
    },
    /// Downloads and sends a GIF as a short looping video.
    SendGif {
        chat: ChatId,
        gif: Gif,
    },
    /// Searches GIPHY or lists trending results for an empty query.
    SearchGifs {
        query: String,
        key: String,
    },
    /// Loads recent and saved stickers for the picker.
    RecentStickers,
    React {
        chat: ChatId,
        message: String,
        emoji: String,
    },
    SetArchived(ChatId, bool),
    SetPinned(ChatId, bool),
    PairWithPhone(String),
    /// Unlinks the device remotely and locally.
    Unlink,
    Reconnect,
    Shutdown,
    /// Internal send result.
    Sent {
        chat: ChatId,
        id: String,
        error: Option<String>,
    },
    /// Internal attachment-download result.
    Downloaded {
        chat: ChatId,
        id: String,
        result: Result<PathBuf, String>,
    },
    /// Internal recent-sticker download result.
    StickerFetched {
        hash: String,
        result: Result<PathBuf, String>,
    },
    /// Internal profile-picture result.
    AvatarFetched {
        id: String,
        full: bool,
        path: Option<PathBuf>,
    },
    /// Internal retryable profile-picture failure.
    AvatarFailed {
        id: String,
        full: bool,
    },
    /// Internal account about-text result.
    MeInfo {
        about: Option<String>,
    },
    /// Internal GIPHY result.
    GifResults {
        query: String,
        results: Result<Vec<Gif>, GifError>,
    },
    /// Internal file-picker result.
    Picked {
        chat: ChatId,
        paths: Vec<PathBuf>,
    },
    /// Internal uploaded attachment ready for archiving and sending.
    Outbound {
        chat: ChatId,
        row: Box<Message>,
        raw: Vec<u8>,
    },
    /// Internal group metadata result.
    GroupInfo {
        chat: ChatId,
        name: Option<String>,
        participants: Vec<String>,
        read_only: bool,
    },
    /// Internal pairing-code result.
    PairCode {
        result: Result<String, String>,
    },
    /// Internal account read-receipt setting.
    ReceiptsPrivacy {
        disabled: bool,
    },
    /// Ask GitHub whether a newer release exists.
    CheckForUpdates,
}

#[derive(Debug)]
pub enum Event {
    Link(LinkStatus),
    /// Linked account identity.
    Me {
        id: String,
        name: Option<String>,
        about: Option<String>,
    },
    /// Full chat list, newest first.
    Chats(Vec<Chat>),
    ChatUpdated(Box<Chat>),
    /// Chat messages in ascending order. `older` prepends them; `complete`
    /// means the archive has no earlier rows.
    Messages {
        chat: ChatId,
        messages: Vec<Message>,
        older: bool,
        complete: bool,
    },
    MessageUpdated(Box<Message>),
    /// Files selected for the composer.
    Picked {
        chat: ChatId,
        paths: Vec<PathBuf>,
    },
    /// Live incoming message for desktop notification.
    Incoming {
        chat: ChatId,
        message: Box<Message>,
    },
    Contacts(Vec<Contact>),
    /// Message search results with their query, newest first.
    SearchHits {
        query: String,
        messages: Vec<Message>,
    },
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
    /// GIF search results or failure.
    Gifs {
        query: String,
        results: Result<Vec<Gif>, GifError>,
    },
    /// Saved stickers, imported packs, and recent stickers for the picker.
    Stickers {
        saved: Vec<PathBuf>,
        packs: Vec<StickerPack>,
        recent: Vec<PathBuf>,
    },
    Media {
        chat: ChatId,
        message: String,
        result: Result<PathBuf, String>,
    },
    /// Link-time history sync state.
    Syncing(bool),
    /// Reported history-sync percentage.
    SyncProgress(u32),
    /// Phone-history result. `more` indicates whether another request may help.
    OlderFetched {
        chat: ChatId,
        more: bool,
    },
    /// Whether account privacy disables direct-chat read receipts.
    ReceiptsPrivacy {
        disabled: bool,
    },
    /// Number lookup succeeded and its chat can open.
    ContactReady {
        id: String,
        name: Option<String>,
    },
    /// Informational toast message.
    Info(String),
    /// A newer release than this build exists.
    UpdateAvailable {
        version: String,
        url: String,
    },
    Error(String),
}

/// Cross-thread window wake handle.
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

    /// Schedules a delayed repaint.
    pub fn wake_after(&self, delay: std::time::Duration) {
        if let Some(ctx) = self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            ctx.request_repaint_after(delay);
        }
    }
}

/// UI handle to the backend runtime.
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

    /// Creates a disconnected backend and event sender for demos and tests.
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

    /// Disables commands except shutdown.
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
