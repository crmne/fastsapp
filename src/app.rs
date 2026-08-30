//! Application state and the frame loop.
//!
//! Views draw from this state and push [`Action`]s; the app applies them
//! after drawing, talks to the backend, and folds its events back in.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::backend::{Backend, Command, Event, LinkStatus, Waker};
use crate::model::{
    Action, Chat, ChatId, Contact, Content, Delivery, Dialog, Gif, Media, MediaState, Message,
    Page, PickerTab, Toast, ToastKind,
};
use crate::paths::AppDirs;
use crate::settings::{Settings, ThemeChoice};
use crate::single_instance::{ControlCommand, Guard};
use crate::theme::Palette;
use crate::tray::{TrayCommand, TrayService};

/// How many messages a chat opens with, and loads per scroll to the top.
pub const PAGE: usize = 60;
/// How long to leave the phone alone after it answered a request for
/// older messages, so scrolling does not hammer it.
const PHONE_COOLDOWN: Duration = Duration::from_secs(6);
/// WhatsApp lets a message be changed for this long after sending.
pub const EDIT_WINDOW: Duration = Duration::from_secs(15 * 60);
/// And taken back from everyone for this long.
pub const REVOKE_WINDOW: Duration = Duration::from_secs(2 * 24 * 60 * 60);

/// A trackpad gesture that pauses this long has ended; the next movement
/// picks its axis afresh.
const SCROLL_GESTURE_GAP: Duration = Duration::from_millis(150);
/// How far short Linux trackpad deltas land of what other apps scroll.
const TRACKPAD_SCALE: f32 = 1.8;
/// The glide's exponential decay time, in seconds; the speed below which a
/// lift starts no glide; and the speed at which a glide stops, points per
/// second.
const GLIDE_DECAY: f32 = 0.35;
const GLIDE_START: f32 = 120.0;
const GLIDE_STOP: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScrollAxis {
    Horizontal,
    Vertical,
}
/// Silence after the last keystroke before the other side is told we
/// stopped typing.
const COMPOSING_TIMEOUT: Duration = Duration::from_secs(4);
/// How long "typing…" stays on screen if the other side never says it
/// stopped.
const TYPING_TIMEOUT: Duration = Duration::from_secs(12);

/// The loaded part of a chat's history.
#[derive(Default)]
pub struct Conversation {
    pub messages: Vec<Message>,
    /// The archive has nothing older than the first message here.
    pub complete: bool,
    pub loading_older: bool,
    /// The first page was asked for.
    pub requested: bool,
    /// The phone is being asked for what came before the archive.
    pub fetching_phone: bool,
    /// The phone said there is nothing older, or cannot be asked.
    pub phone_exhausted: bool,
    /// When the phone last answered, for the cooldown.
    pub phone_answered: Option<Instant>,
    /// Requests the phone answered with nothing, in a row; each doubles
    /// the wait before the next.
    pub phone_misses: u32,
    /// Older messages arrived since the phone was last asked.
    pub phone_delivered: bool,
}

impl Conversation {
    fn merge(&mut self, incoming: Vec<Message>, older: bool) {
        if older {
            let known: HashSet<String> = self.messages.iter().map(|m| m.id.clone()).collect();
            let mut fresh: Vec<Message> = incoming
                .into_iter()
                .filter(|message| !known.contains(&message.id))
                .collect();
            fresh.append(&mut self.messages);
            self.messages = fresh;
        } else {
            for message in incoming {
                match self.messages.iter_mut().find(|m| m.id == message.id) {
                    Some(existing) => *existing = message,
                    None => self.messages.push(message),
                }
            }
        }
        self.messages.sort_by_key(|message| message.timestamp);
    }

    pub fn message_mut(&mut self, id: &str) -> Option<&mut Message> {
        self.messages.iter_mut().find(|message| message.id == id)
    }

    pub fn message(&self, id: &str) -> Option<&Message> {
        self.messages.iter().find(|message| message.id == id)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Presence {
    pub online: bool,
    pub last_seen: Option<i64>,
}

pub struct App {
    pub dirs: AppDirs,
    pub settings: Settings,
    settings_dirty: bool,
    last_settings_save: Instant,
    pub backend: Backend,
    pub palette: Palette,
    applied_dark: Option<bool>,
    zoom_applied: bool,

    pub link: LinkStatus,
    /// History is being replayed after linking.
    pub syncing: bool,
    pub sync_percent: Option<u32>,
    pub me: Option<String>,
    pub me_name: Option<String>,
    /// Our "about" line.
    pub me_about: Option<String>,

    /// Newest activity first, as the backend sends them.
    pub chats: Vec<Chat>,
    pub contacts: HashMap<String, Contact>,
    pub conversations: HashMap<ChatId, Conversation>,
    pub open_chat: Option<ChatId>,
    /// What is typed in each chat's composer, kept while switching chats.
    pub drafts: HashMap<ChatId, String>,
    pub composer: String,
    /// The message the composer is replying to, in the open chat.
    pub reply_to: Option<String>,
    /// The message of ours the composer is changing, in the open chat.
    pub editing: Option<String>,
    composing: bool,
    last_keystroke: Option<Instant>,
    pub search: String,
    /// Who is typing in each chat, and since when.
    pub typing: HashMap<ChatId, Vec<(String, Instant)>>,
    pub presence: HashMap<String, Presence>,
    avatars: HashMap<String, Option<PathBuf>>,
    avatar_requests: HashSet<String>,
    /// The large pictures, for info dialogs.
    avatars_full: HashMap<String, Option<PathBuf>>,
    avatar_full_requests: HashSet<String>,
    /// Files are being dragged over the window.
    pub dropping: bool,
    /// The emoji/GIF/sticker picker, when open.
    pub picker: Option<PickerTab>,
    /// Where the picker hangs from: the composer's smiley button.
    pub picker_anchor: Option<egui::Rect>,
    pub picker_search: String,
    /// The picker just opened: its search field takes the focus once.
    pub picker_focus: bool,
    pub gif_query: String,
    pub gif_results: Vec<Gif>,
    /// A GIF search is on its way.
    pub gif_pending: bool,
    pub gif_error: Option<String>,
    pub stickers: Vec<PathBuf>,
    /// The sticker list was asked for and has not come back yet.
    pub stickers_pending: bool,
    scroll_lock: Option<(ScrollAxis, Instant)>,
    scroll_from_trackpad: bool,
    scroll_history: egui::util::History<egui::Vec2>,
    scroll_accum: egui::Vec2,
    glide: Option<egui::Vec2>,
    scroll_last_event: Option<Instant>,

    pub page: Page,
    pub dialog: Option<Dialog>,
    /// The number typed in the pair-with-phone dialog.
    pub pair_phone: String,
    pub sidebar_visible: bool,
    pub show_archived: bool,
    pub toasts: Vec<Toast>,
    pub actions: Vec<Action>,
    /// The conversation view should show its newest message this frame.
    pub scroll_to_bottom: bool,
    /// The conversation view was at its end last frame, so new messages
    /// may pull it along.
    pub at_bottom: bool,
    /// A message the conversation view should bring into view: the one
    /// that was at the top before older ones were loaded, or a quoted one.
    pub scroll_anchor: Option<String>,
    pub focus_composer: bool,
    pub focus_search: bool,
    pub quit_requested: bool,
    pub window_focused: bool,
    /// Repaints the window when something arrives from another thread.
    waker: Waker,
    tray: Option<TrayService>,
    /// No window exists; the app lives in the tray.
    pub window_hidden: bool,
    /// The window should close but the process should stay in the tray.
    pub hide_intent: bool,
    /// Something asked for a window while there is none.
    pub wants_show: bool,
    /// What other launches asked for, when this process holds the instance.
    control_commands: Option<std::sync::Arc<std::sync::Mutex<Vec<ControlCommand>>>>,
    /// Chats whose notification was clicked.
    notification_opens: std::sync::Arc<std::sync::Mutex<Vec<ChatId>>>,
}

/// What a process wants of the app beyond the window.
#[derive(Clone, Copy, Debug)]
pub struct AppOptions {
    /// Register the system-tray item.
    pub tray: bool,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self { tray: true }
    }
}

impl App {
    pub fn new(waker: &Waker, dirs: AppDirs, settings: Settings, options: AppOptions) -> Self {
        let backend = Backend::spawn(dirs.clone(), waker.clone());
        let mut app = Self::with_backend(dirs, settings, backend, waker.clone());
        if options.tray {
            let waker = waker.clone();
            app.tray = TrayService::spawn(move || waker.wake());
        }
        app
    }

    /// Other launches reach this process through the instance guard.
    pub fn set_remote_control(&mut self, guard: &Guard) {
        self.control_commands = Some(guard.commands());
    }

    /// An app with no connection, for the demo mode and tests. Events can
    /// be fed through the returned sender.
    pub fn headless(dirs: AppDirs, settings: Settings) -> (Self, std::sync::mpsc::Sender<Event>) {
        let (backend, events) = Backend::detached();
        (
            Self::with_backend(dirs, settings, backend, Waker::default()),
            events,
        )
    }

    fn with_backend(dirs: AppDirs, settings: Settings, backend: Backend, waker: Waker) -> Self {
        let palette = match settings.theme {
            ThemeChoice::Light => Palette::light(),
            _ => Palette::dark(),
        };
        let open_chat = settings.last_chat.clone();
        Self {
            dirs,
            settings,
            settings_dirty: false,
            last_settings_save: Instant::now(),
            backend,
            palette,
            applied_dark: None,
            zoom_applied: false,
            link: LinkStatus::Starting,
            syncing: false,
            sync_percent: None,
            me: None,
            me_name: None,
            me_about: None,
            chats: Vec::new(),
            contacts: HashMap::new(),
            conversations: HashMap::new(),
            open_chat,
            drafts: HashMap::new(),
            composer: String::new(),
            reply_to: None,
            editing: None,
            composing: false,
            last_keystroke: None,
            search: String::new(),
            typing: HashMap::new(),
            presence: HashMap::new(),
            avatars: HashMap::new(),
            avatar_requests: HashSet::new(),
            avatars_full: HashMap::new(),
            avatar_full_requests: HashSet::new(),
            dropping: false,
            picker: None,
            picker_anchor: None,
            picker_search: String::new(),
            picker_focus: false,
            gif_query: String::new(),
            gif_results: Vec::new(),
            gif_pending: false,
            gif_error: None,
            stickers: Vec::new(),
            stickers_pending: false,
            scroll_lock: None,
            scroll_from_trackpad: false,
            scroll_history: egui::util::History::new(2..16, 0.1),
            scroll_accum: egui::Vec2::ZERO,
            glide: None,
            scroll_last_event: None,
            page: Page::Chats,
            dialog: None,
            pair_phone: String::new(),
            sidebar_visible: true,
            show_archived: false,
            toasts: Vec::new(),
            actions: Vec::new(),
            scroll_to_bottom: true,
            at_bottom: true,
            scroll_anchor: None,
            focus_composer: false,
            focus_search: false,
            quit_requested: false,
            window_focused: true,
            waker,
            tray: None,
            window_hidden: false,
            hide_intent: false,
            wants_show: false,
            control_commands: None,
            notification_opens: Default::default(),
        }
    }

    /// The window is gone but the process stays: the link, the archive,
    /// and the tray keep going until Show or Quit.
    pub fn window_gone(&mut self) {
        self.window_hidden = true;
        self.hide_intent = false;
        self.wants_show = false;
        if let Some(tray) = &mut self.tray {
            tray.hidden();
        }
    }

    /// Whether closing the window keeps the app in the tray rather than
    /// quitting.
    pub fn hides_to_tray(&self) -> bool {
        self.tray.is_some() && self.settings.keep_running_in_background
    }

    fn handle_tray(&mut self) {
        let Some(commands) = self.tray.as_ref().map(TrayService::drain_commands) else {
            return;
        };
        for command in commands {
            match command {
                TrayCommand::Show => self.actions.push(Action::ShowWindow),
                TrayCommand::ShowHide => self.actions.push(if self.window_hidden {
                    Action::ShowWindow
                } else {
                    Action::HideWindow
                }),
                TrayCommand::Quit => self.actions.push(Action::Quit),
            }
        }
    }

    fn handle_control_commands(&mut self) {
        let Some(queue) = &self.control_commands else {
            return;
        };
        let commands: Vec<ControlCommand> =
            std::mem::take(&mut *queue.lock().unwrap_or_else(|p| p.into_inner()));
        for command in commands {
            match command {
                ControlCommand::Show => self.actions.push(Action::ShowWindow),
            }
        }
    }

    /// A clicked notification opens its chat, in a window if need be.
    fn handle_notification_opens(&mut self) {
        let opened: Vec<ChatId> = std::mem::take(
            &mut *self
                .notification_opens
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        );
        for chat in opened {
            self.actions.push(Action::OpenChat(chat));
            self.actions.push(Action::ShowWindow);
        }
    }

    /// Tells the desktop about a message that arrived while the reader was
    /// not looking at that chat.
    fn maybe_notify(&self, chat_id: &str, message: &Message) {
        if !self.settings.notifications {
            return;
        }
        let Some(chat) = self.chat(chat_id) else {
            return;
        };
        let now = crate::util::now();
        // Muted chats stay quiet, and a backlog drained on reconnect is not
        // news.
        if chat.muted(now) || now - message.timestamp > 60 {
            return;
        }
        let reading = !self.window_hidden
            && self.window_focused
            && self.page == Page::Chats
            && self.open_chat.as_deref() == Some(chat_id);
        if reading {
            return;
        }
        let sender = self.display_name(&message.sender);
        let (title, body) =
            crate::notify::lines(&chat.name, chat.is_group(), &sender, &message.summary());
        let waker = self.waker.clone();
        crate::notify::show(
            title,
            body,
            chat_id.to_owned(),
            std::sync::Arc::clone(&self.notification_opens),
            move || waker.wake(),
        );
    }

    /// Once per window, before the first frame.
    pub fn attach(&mut self, ctx: &egui::Context) {
        crate::theme::install(ctx);
        // Three lines per wheel notch is what egui ships; a chat of short
        // rows wants the pace every other client scrolls at.
        ctx.options_mut(|options| options.input_options.line_scroll_speed = 120.0);
        // The colour emoji font is a few megabytes to read and index; do
        // it while the first frames go by rather than in one of them.
        std::thread::Builder::new()
            .name("emoji-font".into())
            .spawn(crate::emoji::warm_up)
            .ok();
        self.applied_dark = None;
        self.zoom_applied = false;
        self.window_hidden = false;
        self.hide_intent = false;
        self.wants_show = false;
        if let Some(tray) = &mut self.tray {
            tray.attach();
        }
    }

    pub fn is_connected(&self) -> bool {
        self.link.is_connected()
    }

    /// The device is linked: history may be showing even while offline.
    pub fn is_linked(&self) -> bool {
        matches!(
            self.link,
            LinkStatus::Connected | LinkStatus::Connecting | LinkStatus::Disconnected { .. }
        ) || (!self.chats.is_empty() && !matches!(self.link, LinkStatus::LoggedOut))
    }

    pub fn chat(&self, id: &str) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.id == id)
    }

    pub fn chat_mut(&mut self, id: &str) -> Option<&mut Chat> {
        self.chats.iter_mut().find(|chat| chat.id == id)
    }

    pub fn current_chat(&self) -> Option<&Chat> {
        self.open_chat.as_deref().and_then(|id| self.chat(id))
    }

    /// The name to show for a sender: the address book's, the name they
    /// chose for themselves (with a tilde, as WhatsApp does), the phone
    /// number, or "Unknown".
    pub fn display_name(&self, id: &str) -> String {
        if self.me.as_deref() == Some(id) {
            return "You".to_owned();
        }
        if let Some(name) = self.contacts.get(id).and_then(Contact::label) {
            return name;
        }
        if let Some(chat) = self.chat(id)
            && !chat.name.is_empty()
            && !chat.name.chars().all(|c| c.is_ascii_digit())
        {
            return chat.name.clone();
        }
        match crate::model::phone_of(id) {
            Some(digits) => crate::util::phone(digits),
            None => "Unknown".to_owned(),
        }
    }

    /// Whether a direct chat's name comes from the address book, rather
    /// than from what the other side calls themselves.
    pub fn is_saved_contact(&self, id: &str) -> bool {
        self.contacts.get(id).is_some_and(|contact| {
            contact
                .full_name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
        })
    }

    /// The members of a group as WhatsApp lists them under its name: first
    /// names, then the numbers of people without one, ourselves last.
    pub fn participant_names(&self, chat: &Chat) -> String {
        let me = self.me.as_deref();
        let mut names = Vec::new();
        let mut numbers = Vec::new();
        for id in chat
            .participants
            .iter()
            .filter(|id| Some(id.as_str()) != me)
        {
            let name = self.display_name(id);
            if name.starts_with('+') || name == "Unknown" {
                numbers.push(name);
            } else {
                let name = name.trim_start_matches('~');
                names.push(name.split_whitespace().next().unwrap_or(name).to_owned());
            }
        }
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        numbers.sort();
        numbers.dedup();
        names.extend(numbers);
        if chat.participants.iter().any(|id| Some(id.as_str()) == me) {
            names.push("You".to_owned());
        }
        names.join(", ")
    }

    /// The chats the list shows: matching the search, archived or not,
    /// pinned first.
    pub fn visible_chats(&self) -> Vec<&Chat> {
        let needle = self.search.trim().to_lowercase();
        let mut chats: Vec<&Chat> = self
            .chats
            .iter()
            .filter(|chat| chat.archived == self.show_archived || !needle.is_empty())
            .filter(|chat| {
                needle.is_empty()
                    || chat.name.to_lowercase().contains(&needle)
                    || chat.phone().is_some_and(|phone| phone.contains(&needle))
                    || chat
                        .last
                        .as_ref()
                        .is_some_and(|last| last.summary.to_lowercase().contains(&needle))
            })
            .collect();
        chats.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.last_activity.cmp(&a.last_activity))
        });
        chats
    }

    pub fn archived_count(&self) -> usize {
        self.chats.iter().filter(|chat| chat.archived).count()
    }

    pub fn unread_total(&self) -> u32 {
        self.chats
            .iter()
            .filter(|chat| !chat.archived && !chat.muted(crate::util::now()))
            .map(|chat| chat.unread)
            .sum()
    }

    /// The profile picture of a chat or contact, asking the backend for it
    /// the first time.
    pub fn avatar(&mut self, id: &str) -> Option<PathBuf> {
        if let Some(known) = self.avatars.get(id) {
            return known.clone();
        }
        if self.avatar_requests.insert(id.to_owned()) {
            self.backend.send(Command::FetchAvatar {
                id: id.to_owned(),
                full: false,
            });
        }
        None
    }

    /// The large profile picture of a chat or contact, asking the backend
    /// for it the first time.
    pub fn avatar_full(&mut self, id: &str) -> Option<PathBuf> {
        if let Some(known) = self.avatars_full.get(id) {
            return known.clone();
        }
        if self.avatar_full_requests.insert(id.to_owned()) {
            self.backend.send(Command::FetchAvatar {
                id: id.to_owned(),
                full: true,
            });
        }
        None
    }

    /// Whether a message of ours is still young enough to change.
    pub fn can_edit(&self, message: &Message) -> bool {
        message.from_me
            && matches!(message.content, Content::Text { .. })
            && crate::util::now() - message.timestamp <= EDIT_WINDOW.as_secs() as i64
    }

    /// Whether a message of ours can still be taken back from everyone.
    pub fn can_revoke(&self, message: &Message) -> bool {
        message.from_me
            && !matches!(message.content, Content::Revoked)
            && crate::util::now() - message.timestamp <= REVOKE_WINDOW.as_secs() as i64
    }

    /// Who is typing in a chat right now, by name.
    pub fn typing_in(&self, chat: &str) -> Vec<String> {
        self.typing
            .get(chat)
            .map(|typers| {
                typers
                    .iter()
                    .map(|(sender, _)| self.display_name(sender))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn handle_events(&mut self) {
        for event in self.backend.poll() {
            match event {
                Event::Link(status) => self.handle_link(status),
                Event::Me { id, name, about } => {
                    self.me = Some(id);
                    self.me_name = name;
                    self.me_about = about;
                }
                Event::Chats(chats) => {
                    self.chats = chats;
                    if let Some(open) = self.open_chat.clone() {
                        if self.chat(&open).is_none() {
                            self.open_chat = None;
                        } else {
                            // The chat kept from last time shows its
                            // archive right away, connected or not.
                            self.ensure_loaded(&open);
                        }
                    }
                }
                Event::ChatUpdated(chat) => self.handle_chat_updated(*chat),
                Event::Messages {
                    chat,
                    messages,
                    older,
                    complete,
                } => {
                    let conversation = self.conversations.entry(chat.clone()).or_default();
                    let was_empty = conversation.messages.is_empty();
                    if older && !messages.is_empty() {
                        conversation.phone_delivered = true;
                    }
                    conversation.merge(messages, older);
                    if older {
                        conversation.loading_older = false;
                        conversation.complete = complete;
                    } else if was_empty {
                        conversation.complete = complete;
                    }
                    if self.open_chat.as_deref() == Some(chat.as_str())
                        && !older
                        && (self.at_bottom || was_empty)
                    {
                        self.scroll_to_bottom = true;
                    }
                }
                Event::Incoming { chat, message } => self.maybe_notify(&chat, &message),
                Event::MessageUpdated(message) => {
                    let message = *message;
                    if let Some(conversation) = self.conversations.get_mut(&message.chat)
                        && let Some(existing) = conversation.message_mut(&message.id)
                    {
                        let state = existing.content.media().map(|media| media.state.clone());
                        *existing = message;
                        if let (Some(state), Some(media)) = (state, existing.content.media_mut()) {
                            media.state = state;
                        }
                    }
                }
                Event::Contacts(contacts) => {
                    for contact in contacts {
                        self.contacts.insert(contact.id.clone(), contact);
                    }
                }
                Event::Typing {
                    chat,
                    sender,
                    composing,
                } => {
                    let typers = self.typing.entry(chat).or_default();
                    typers.retain(|(who, _)| *who != sender);
                    if composing {
                        typers.push((sender, Instant::now()));
                    }
                }
                Event::Presence {
                    id,
                    online,
                    last_seen,
                } => {
                    self.presence.insert(id, Presence { online, last_seen });
                }
                Event::Avatar { id, full, path } => {
                    if full {
                        self.avatar_full_requests.remove(&id);
                        self.avatars_full.insert(id, path);
                    } else {
                        self.avatar_requests.remove(&id);
                        self.avatars.insert(id, path);
                    }
                }
                Event::Gifs { query, results } => {
                    if query == self.gif_query {
                        self.gif_pending = false;
                        match results {
                            Ok(results) => {
                                self.gif_results = results;
                                self.gif_error = None;
                            }
                            Err(error) => {
                                self.gif_results.clear();
                                self.gif_error = Some(error);
                            }
                        }
                    }
                }
                Event::Stickers(stickers) => {
                    self.stickers = stickers;
                    self.stickers_pending = false;
                }
                Event::MessageDeleted { chat, id } => {
                    if let Some(conversation) = self.conversations.get_mut(&chat) {
                        conversation.messages.retain(|message| message.id != id);
                    }
                    if self.editing.as_deref() == Some(id.as_str()) {
                        self.editing = None;
                        self.composer.clear();
                    }
                }
                Event::Media {
                    chat,
                    message,
                    result,
                } => self.handle_media(&chat, &message, result),
                Event::Syncing(syncing) => {
                    if self.syncing && !syncing {
                        self.toast("History is in");
                    }
                    self.syncing = syncing;
                    if !syncing {
                        self.sync_percent = None;
                    }
                }
                Event::SyncProgress(percent) => self.sync_percent = Some(percent),
                Event::OlderFetched { chat, more } => {
                    let conversation = self.conversations.entry(chat).or_default();
                    conversation.fetching_phone = false;
                    conversation.phone_exhausted = !more;
                    conversation.phone_answered = Some(Instant::now());
                    if conversation.phone_delivered {
                        conversation.phone_misses = 0;
                    } else {
                        conversation.phone_misses = (conversation.phone_misses + 1).min(7);
                    }
                    conversation.phone_delivered = false;
                    // Whatever the phone sent is in the archive, even when
                    // it came late: page the archive again before asking.
                    conversation.complete = false;
                }
                Event::Error(message) => self.toast_error(message),
            }
        }
    }

    fn handle_link(&mut self, status: LinkStatus) {
        match &status {
            LinkStatus::Connected => {
                if matches!(self.link, LinkStatus::Disconnected { .. }) {
                    self.toast("Back online");
                }
                self.dialog = match self.dialog.take() {
                    Some(Dialog::PairWithPhone) => None,
                    other => other,
                };
                if let Some(open) = self.open_chat.clone() {
                    self.ensure_loaded(&open);
                }
            }
            LinkStatus::LoggedOut => {
                self.chats.clear();
                self.conversations.clear();
                self.contacts.clear();
                self.avatars.clear();
                self.open_chat = None;
                self.toast_error("This device was unlinked from your phone");
            }
            LinkStatus::Failed(message) => self.toast_error(message.clone()),
            _ => {}
        }
        self.link = status;
    }

    fn handle_chat_updated(&mut self, chat: Chat) {
        let is_open =
            self.open_chat.as_deref() == Some(chat.id.as_str()) && self.page == Page::Chats;
        let mut chat = chat;
        if is_open && chat.unread > 0 && self.window_focused {
            chat.unread = 0;
            self.mark_read(&chat.id);
        }
        match self.chats.iter_mut().find(|known| known.id == chat.id) {
            Some(existing) => *existing = chat,
            None => self.chats.push(chat),
        }
        self.chats
            .sort_by_key(|chat| std::cmp::Reverse(chat.last_activity));
    }

    fn handle_media(&mut self, chat: &str, id: &str, result: Result<PathBuf, String>) {
        let Some(message) = self
            .conversations
            .get_mut(chat)
            .and_then(|conversation| conversation.message_mut(id))
        else {
            return;
        };
        let Some(media) = message.content.media_mut() else {
            return;
        };
        match result {
            Ok(path) => {
                media.path = Some(path);
                media.state = MediaState::Idle;
            }
            Err(error) => {
                media.state = MediaState::Failed(error.clone());
                // WhatsApp keeps a file for a while only; a 403 or 404 is the
                // server saying it is gone, not a fault here.
                let notice = if error.contains("403") || error.contains("404") {
                    "This file is no longer on WhatsApp's servers; ask for it to be sent again"
                        .to_owned()
                } else {
                    format!("Download failed: {error}")
                };
                self.toast_error(notice);
            }
        }
    }

    fn ensure_loaded(&mut self, chat: &str) {
        let conversation = self.conversations.entry(chat.to_owned()).or_default();
        if !conversation.requested {
            conversation.requested = true;
            self.backend.send(Command::LoadChat {
                chat: chat.to_owned(),
                before: None,
            });
        }
    }

    pub fn load_older(&mut self, chat: &str) {
        let Some(conversation) = self.conversations.get_mut(chat) else {
            return;
        };
        if conversation.loading_older {
            return;
        }
        let Some(oldest) = conversation.messages.first() else {
            return;
        };
        if conversation.complete {
            self.fetch_older(chat);
            return;
        }
        conversation.loading_older = true;
        let before = (oldest.timestamp, oldest.id.clone());
        self.scroll_anchor = Some(oldest.id.clone());
        self.backend.send(Command::LoadChat {
            chat: chat.to_owned(),
            before: Some(before),
        });
    }

    /// Asks the phone for what came before the archive, if it has not said
    /// there is nothing and was not asked a moment ago.
    pub fn fetch_older(&mut self, chat: &str) {
        let Some(conversation) = self.conversations.get_mut(chat) else {
            return;
        };
        if conversation.fetching_phone || conversation.phone_exhausted {
            return;
        }
        // Only a linked, connected phone can answer; asking again after an
        // empty answer waits twice as long each time.
        if !matches!(self.link, LinkStatus::Connected) {
            return;
        }
        let cooldown =
            (PHONE_COOLDOWN * 2u32.pow(conversation.phone_misses)).min(Duration::from_secs(600));
        if conversation
            .phone_answered
            .is_some_and(|answered| answered.elapsed() < cooldown)
        {
            return;
        }
        let Some(oldest) = conversation.messages.first() else {
            return;
        };
        conversation.fetching_phone = true;
        self.scroll_anchor = Some(oldest.id.clone());
        self.backend.send(Command::FetchOlder(chat.to_owned()));
    }

    fn mark_read(&mut self, chat: &str) {
        if let Some(known) = self.chat_mut(chat) {
            known.unread = 0;
        }
        // The archive's count clears either way; receipts are a setting.
        self.backend.send(Command::MarkRead {
            chat: chat.to_owned(),
            receipts: self.settings.send_read_receipts,
        });
    }

    fn open_chat(&mut self, id: ChatId) {
        if self.open_chat.as_deref() != Some(id.as_str()) {
            if let Some(previous) = self.open_chat.take() {
                let draft = std::mem::take(&mut self.composer);
                // An edit in progress is dropped, not kept as a draft that
                // would send the old text again.
                if self.editing.take().is_some() || draft.trim().is_empty() {
                    self.drafts.remove(&previous);
                } else {
                    self.drafts.insert(previous.clone(), draft);
                }
                self.stop_composing(&previous);
            }
            self.composer = self.drafts.remove(&id).unwrap_or_default();
            self.reply_to = None;
            self.editing = None;
        }
        self.open_chat = Some(id.clone());
        self.page = Page::Chats;
        self.scroll_to_bottom = true;
        self.at_bottom = true;
        self.focus_composer = true;
        self.ensure_loaded(&id);
        if self.chat(&id).is_some_and(|chat| chat.unread > 0) {
            self.mark_read(&id);
        }
        if self.settings.last_chat.as_deref() != Some(id.as_str()) {
            self.settings.last_chat = Some(id);
            self.mark_settings_dirty();
        }
    }

    /// The composer changed: tell the other side we are typing, once, and
    /// again after a pause.
    pub fn note_keystroke(&mut self) {
        self.last_keystroke = Some(Instant::now());
        if !self.composing
            && self.settings.send_typing
            && let Some(chat) = self.open_chat.clone()
        {
            self.composing = true;
            self.backend.send(Command::Composing {
                chat,
                composing: true,
            });
        }
    }

    fn stop_composing(&mut self, chat: &str) {
        if self.composing {
            self.composing = false;
            self.backend.send(Command::Composing {
                chat: chat.to_owned(),
                composing: false,
            });
        }
        self.last_keystroke = None;
    }

    fn send_text(&mut self, chat: ChatId, text: String, quoting: Option<String>) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.stop_composing(&chat);
        if let Some(id) = self.editing.take() {
            if let Some(message) = self
                .conversations
                .get_mut(&chat)
                .and_then(|conversation| conversation.message_mut(&id))
            {
                message.content = Content::text(text.clone());
                message.edited = true;
            }
            self.backend.send(Command::EditText { chat, id, text });
            return;
        }
        self.backend.send(Command::SendText {
            chat,
            text,
            quoting,
        });
        self.scroll_to_bottom = true;
        self.at_bottom = true;
    }

    /// Sends files to the open chat.
    fn send_files(&mut self, paths: Vec<PathBuf>) {
        let Some(chat) = self.open_chat.clone() else {
            self.toast_error("Open a chat first");
            return;
        };
        if paths.is_empty() {
            return;
        }
        self.toast(format!(
            "Sending {} file{}…",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ));
        self.backend.send(Command::SendFiles { chat, paths });
        self.scroll_to_bottom = true;
        self.at_bottom = true;
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if self.composing
            && let Some(last) = self.last_keystroke
            && now.duration_since(last) > COMPOSING_TIMEOUT
            && let Some(chat) = self.open_chat.clone()
        {
            self.stop_composing(&chat);
        }
        for typers in self.typing.values_mut() {
            typers.retain(|(_, since)| now.duration_since(*since) < TYPING_TIMEOUT);
        }
        self.typing.retain(|_, typers| !typers.is_empty());
        self.toasts
            .retain(|toast| toast.created.elapsed() < Duration::from_millis(3200));
        if self.settings_dirty && self.last_settings_save.elapsed() > Duration::from_secs(2) {
            self.save_settings();
        }
        if !self.typing.is_empty() || self.composing {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    pub fn mark_settings_dirty(&mut self) {
        self.settings_dirty = true;
    }

    fn save_settings(&mut self) {
        self.settings_dirty = false;
        self.last_settings_save = Instant::now();
        if let Err(error) = self.settings.save(&self.dirs.settings_file()) {
            log::warn!("could not save settings: {error}");
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let dark = match self.settings.theme {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => ctx
                .input(|input| input.raw.system_theme)
                .is_none_or(|theme| theme == egui::Theme::Dark),
        };
        if self.applied_dark != Some(dark) {
            self.palette = if dark {
                Palette::dark()
            } else {
                Palette::light()
            };
            crate::theme::apply(ctx, &self.palette);
            self.applied_dark = Some(dark);
        }
        if !self.zoom_applied {
            ctx.set_zoom_factor(self.settings.zoom);
            self.zoom_applied = true;
        }
    }

    fn apply_actions(&mut self, ctx: &egui::Context) {
        let mut actions = std::mem::take(&mut self.actions);
        while !actions.is_empty() {
            for action in actions.drain(..) {
                self.apply(action, ctx);
            }
            actions = std::mem::take(&mut self.actions);
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Open(page) => {
                self.page = page;
                self.dialog = None;
            }
            Action::OpenChat(id) => self.open_chat(id),
            Action::CloseChat => {
                if let Some(chat) = self.open_chat.take() {
                    self.stop_composing(&chat);
                    let draft = std::mem::take(&mut self.composer);
                    if self.editing.take().is_none() && !draft.trim().is_empty() {
                        self.drafts.insert(chat, draft);
                    }
                }
                self.reply_to = None;
            }
            Action::SendText {
                chat,
                text,
                quoting,
            } => {
                self.send_text(chat, text, quoting);
                self.reply_to = None;
            }
            Action::Composing { chat, composing } => {
                if composing {
                    self.note_keystroke();
                } else {
                    self.stop_composing(&chat);
                }
            }
            Action::MarkRead(chat) => self.mark_read(&chat),
            Action::LoadOlder(chat) => self.load_older(&chat),
            Action::FetchOlder(chat) => self.fetch_older(&chat),
            Action::Download { chat, message } => {
                if let Some(media) = self
                    .conversations
                    .get_mut(&chat)
                    .and_then(|conversation| conversation.message_mut(&message))
                    .and_then(|message| message.content.media_mut())
                {
                    media.state = MediaState::Downloading;
                }
                self.backend.send(Command::Download { chat, message });
            }
            Action::OpenFile(path) => {
                if let Err(error) = open::that_detached(&path) {
                    self.toast_error(format!("Could not open {}: {error}", path.display()));
                }
            }
            Action::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Action::CopyText(text) => {
                ctx.copy_text(text);
                self.toast("Copied");
            }
            Action::Reply(id) => {
                self.reply_to = Some(id);
                self.focus_composer = true;
            }
            Action::CancelReply => self.reply_to = None,
            Action::Edit(id) => {
                let text = self
                    .open_chat
                    .as_deref()
                    .and_then(|chat| self.conversations.get(chat))
                    .and_then(|conversation| conversation.message(&id))
                    .and_then(|message| match &message.content {
                        Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    });
                if let Some(text) = text {
                    self.editing = Some(id);
                    self.reply_to = None;
                    self.composer = text;
                    self.focus_composer = true;
                }
            }
            Action::CancelEdit => {
                if self.editing.take().is_some() {
                    self.composer.clear();
                }
            }
            Action::DeleteForEveryone(id) => {
                if let Some(chat) = self.open_chat.clone() {
                    if let Some(message) = self
                        .conversations
                        .get_mut(&chat)
                        .and_then(|conversation| conversation.message_mut(&id))
                    {
                        message.content = Content::Revoked;
                    }
                    self.backend.send(Command::Revoke { chat, id });
                }
            }
            Action::DeleteForMe(id) => {
                if let Some(chat) = self.open_chat.clone() {
                    if let Some(conversation) = self.conversations.get_mut(&chat) {
                        conversation.messages.retain(|message| message.id != id);
                    }
                    self.backend.send(Command::DeleteLocal { chat, id });
                }
            }
            Action::Attach => {
                if let Some(chat) = self.open_chat.clone() {
                    self.backend.send(Command::PickFiles(chat));
                }
            }
            Action::SendFiles(paths) => self.send_files(paths),
            Action::TogglePicker(tab) => {
                if self.picker == Some(tab) {
                    self.picker = None;
                } else {
                    self.picker = Some(tab);
                    self.picker_search.clear();
                    self.picker_focus = tab == PickerTab::Emoji;
                    if tab == PickerTab::Stickers {
                        self.stickers_pending = self.stickers.is_empty();
                        self.backend.send(Command::RecentStickers);
                    }
                    if tab == PickerTab::Gifs && self.gif_results.is_empty() {
                        self.actions.push(Action::SearchGifs(String::new()));
                    }
                }
            }
            Action::ClosePicker => self.picker = None,
            Action::InsertEmoji(emoji) => {
                self.insert_in_composer(ctx, &emoji);
                self.settings.recent_emoji.retain(|known| *known != emoji);
                self.settings.recent_emoji.insert(0, emoji);
                self.settings.recent_emoji.truncate(36);
                self.mark_settings_dirty();
                self.focus_composer = true;
            }
            Action::SendSticker(path) => {
                if let Some(chat) = self.open_chat.clone() {
                    self.backend.send(Command::SendSticker { chat, path });
                    self.picker = None;
                    self.scroll_to_bottom = true;
                    self.at_bottom = true;
                }
            }
            Action::SearchGifs(query) => {
                self.gif_query = query.clone();
                self.gif_pending = true;
                self.gif_error = None;
                self.backend.send(Command::SearchGifs {
                    query,
                    key: self.settings.effective_giphy_key().unwrap_or_default(),
                });
            }
            Action::SendGif(gif) => {
                if let Some(chat) = self.open_chat.clone() {
                    self.toast("Sending the GIF…");
                    self.backend.send(Command::SendGif { chat, gif });
                    self.picker = None;
                    self.scroll_to_bottom = true;
                    self.at_bottom = true;
                }
            }
            Action::PasteImage {
                width,
                height,
                rgba,
            } => {
                if let Some(chat) = self.open_chat.clone() {
                    self.toast("Sending the picture…");
                    self.backend.send(Command::SendImage {
                        chat,
                        width: width as u32,
                        height: height as u32,
                        rgba,
                    });
                    self.scroll_to_bottom = true;
                    self.at_bottom = true;
                }
            }
            Action::React {
                chat,
                message,
                emoji,
            } => self.backend.send(Command::React {
                chat,
                message,
                emoji,
            }),
            Action::SetArchived(chat, archived) => {
                if let Some(known) = self.chat_mut(&chat) {
                    known.archived = archived;
                }
                if archived && self.open_chat.as_deref() == Some(chat.as_str()) {
                    self.actions.push(Action::CloseChat);
                }
                self.backend.send(Command::SetArchived(chat, archived));
            }
            Action::SetPinned(chat, pinned) => {
                if let Some(known) = self.chat_mut(&chat) {
                    known.pinned = pinned;
                }
                self.backend.send(Command::SetPinned(chat, pinned));
            }
            Action::ShowDialog(dialog) => {
                if dialog == Dialog::PairWithPhone {
                    self.pair_phone.clear();
                }
                self.dialog = Some(dialog);
            }
            Action::CloseDialog => self.dialog = None,
            Action::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            Action::FocusSearch => {
                self.sidebar_visible = true;
                self.page = Page::Chats;
                self.focus_search = true;
            }
            Action::FocusComposer => self.focus_composer = true,
            Action::ScrollToBottom => self.scroll_to_bottom = true,
            Action::ScrollTo(id) => {
                self.scroll_to_bottom = false;
                let Some(chat) = self.open_chat.clone() else {
                    return;
                };
                let conversation = self.conversations.entry(chat.clone()).or_default();
                if conversation.message(&id).is_none()
                    && !conversation.loading_older
                    && let Some(oldest) = conversation.messages.first()
                {
                    // Older than what is loaded: bring the archive up to it.
                    conversation.loading_older = true;
                    self.backend.send(Command::LoadUntil {
                        chat,
                        id: id.clone(),
                        before: (oldest.timestamp, oldest.id.clone()),
                    });
                }
                self.scroll_anchor = Some(id);
            }
            Action::Search(text) => self.search = text,
            Action::SettingsChanged => self.mark_settings_dirty(),
            Action::ZoomBy(delta) => {
                self.settings.zoom = (self.settings.zoom + delta).clamp(0.6, 2.0);
                self.zoom_applied = false;
                self.mark_settings_dirty();
            }
            Action::ResetZoom => {
                self.settings.zoom = 1.0;
                self.zoom_applied = false;
                self.mark_settings_dirty();
            }
            Action::PairWithPhone(phone) => {
                let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
                if digits.len() < 7 {
                    self.toast_error("Enter the number with its country code, digits only");
                } else {
                    self.backend.send(Command::PairWithPhone(digits));
                }
            }
            Action::Unlink => {
                self.dialog = None;
                self.backend.send(Command::Unlink);
            }
            Action::Reconnect => self.backend.send(Command::Reconnect),
            Action::Quit => {
                self.quit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::ShowWindow => {
                if self.window_hidden {
                    // No window exists; the loop in `main` creates one.
                    self.wants_show = true;
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
            Action::HideWindow => {
                if self.tray.is_some() {
                    self.hide_intent = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    pub fn toast(&mut self, message: impl Into<String>) {
        self.toasts.push(Toast {
            message: message.into(),
            kind: ToastKind::Info,
            created: Instant::now(),
        });
        self.toasts.truncate(4);
    }

    pub fn toast_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::warn!("{message}");
        self.toasts.push(Toast {
            message,
            kind: ToastKind::Error,
            created: Instant::now(),
        });
    }

    /// Everything that happens whether or not the window is showing.
    pub fn background_frame(&mut self, ctx: &egui::Context) {
        self.handle_tray();
        self.handle_control_commands();
        self.handle_notification_opens();
        self.handle_events();
        self.tick(ctx);
        self.apply_actions(ctx);
    }

    pub fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.apply_theme(ctx);
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        // Coming back to the window is reading what arrived meanwhile.
        if focused
            && !self.window_focused
            && self.page == Page::Chats
            && let Some(open) = self.open_chat.clone()
            && self.chat(&open).is_some_and(|chat| chat.unread > 0)
        {
            self.mark_read(&open);
        }
        self.window_focused = focused;
        // Closing the window keeps the app in the tray when asked to; the
        // window still goes, and the loop in `main` carries on without it.
        if ctx.input(|input| input.viewport().close_requested())
            && !self.quit_requested
            && self.hides_to_tray()
        {
            self.hide_intent = true;
        }
        self.lock_scroll_axis(ctx);
        self.take_drops_and_pastes(ctx);
        crate::ui::show(self, ui);
        self.apply_actions(ctx);
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    /// Puts text into the composer at its cursor, or at the end.
    fn insert_in_composer(&mut self, ctx: &egui::Context, text: &str) {
        let id = egui::Id::new("composer-text");
        let at = egui::TextEdit::load_state(ctx, id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index.0)
            .unwrap_or_else(|| self.composer.chars().count());
        let at = at.min(self.composer.chars().count());
        let byte = self
            .composer
            .char_indices()
            .nth(at)
            .map_or(self.composer.len(), |(byte, _)| byte);
        self.composer.insert_str(byte, text);
        if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
            let after = at + text.chars().count();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(after),
                )));
            egui::TextEdit::store_state(ctx, id, state);
        }
    }

    /// Files dragged onto the window go to the open chat; a picture pasted
    /// while writing does too.
    fn take_drops_and_pastes(&mut self, ctx: &egui::Context) {
        let (dropped, hovering, paste) = ctx.input(|input| {
            let dropped: Vec<PathBuf> = input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect();
            let hovering = !input.raw.hovered_files.is_empty();
            let paste = input.modifiers.command && input.key_pressed(egui::Key::V);
            (dropped, hovering, paste)
        });
        self.dropping = hovering && self.open_chat.is_some();
        if !dropped.is_empty() {
            self.actions.push(Action::SendFiles(dropped));
        }
        let composing = ctx.memory(|memory| memory.has_focus(egui::Id::new("composer-text")));
        if paste && composing && self.open_chat.is_some() {
            // egui pastes text on its own; a picture on the clipboard is
            // ours to notice.
            if let Some(image) = clipboard_image() {
                self.actions.push(Action::PasteImage {
                    width: image.0,
                    height: image.1,
                    rgba: image.2,
                });
            }
        }
    }

    /// Keeps a scroll gesture on one axis, scales Linux trackpad deltas up
    /// to what other apps scroll, and lets a lifted gesture glide.
    fn lock_scroll_axis(&mut self, ctx: &egui::Context) {
        let (raw, from_trackpad, ended) = ctx.input(|input| {
            let mut sum = egui::Vec2::ZERO;
            let mut pointish = false;
            let mut ended = false;
            for event in &input.events {
                if let egui::Event::MouseWheel {
                    unit, delta, phase, ..
                } = event
                {
                    sum += *delta;
                    pointish |= *unit == egui::MouseWheelUnit::Point;
                    ended |= matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel);
                }
            }
            (sum, pointish, ended)
        });
        let now = Instant::now();
        if raw != egui::Vec2::ZERO {
            self.scroll_from_trackpad = from_trackpad;
        }
        let trackpad_here = cfg!(target_os = "linux") && self.scroll_from_trackpad;
        if trackpad_here {
            ctx.input_mut(|input| input.smooth_scroll_delta *= TRACKPAD_SCALE);
        }
        if trackpad_here && raw != egui::Vec2::ZERO {
            self.glide = None;
            self.scroll_accum += raw * TRACKPAD_SCALE;
            self.scroll_history
                .add(ctx.input(|input| input.time), self.scroll_accum);
            self.scroll_last_event = Some(now);
            ctx.request_repaint_after(Duration::from_millis(60));
        } else if raw != egui::Vec2::ZERO || ctx.input(|input| input.pointer.any_down()) {
            self.glide = None;
            self.scroll_history.clear();
            self.scroll_last_event = None;
        }
        let quiet = self
            .scroll_last_event
            .is_some_and(|at| now.duration_since(at).as_secs_f32() > 0.15);
        if ended || quiet {
            let mut velocity = self.scroll_history.velocity().unwrap_or(egui::Vec2::ZERO);
            if let Some((axis, _)) = self.scroll_lock {
                match axis {
                    ScrollAxis::Horizontal => velocity.y = 0.0,
                    ScrollAxis::Vertical => velocity.x = 0.0,
                }
            }
            self.glide = (velocity.length() > GLIDE_START).then_some(velocity);
            self.scroll_history.clear();
            self.scroll_accum = egui::Vec2::ZERO;
            self.scroll_last_event = None;
        }
        if let Some(velocity) = self.glide {
            if raw == egui::Vec2::ZERO {
                let dt = ctx.input(|input| input.stable_dt).clamp(0.001, 0.05);
                ctx.input_mut(|input| input.smooth_scroll_delta += velocity * dt);
                let slower = velocity * (-dt / GLIDE_DECAY).exp();
                self.glide = (slower.length() > GLIDE_STOP).then_some(slower);
            }
            ctx.request_repaint();
        }
        let held = self
            .scroll_lock
            .filter(|(_, at)| now.duration_since(*at) < SCROLL_GESTURE_GAP)
            .map(|(axis, _)| axis);
        let moved = raw != egui::Vec2::ZERO;
        let axis = match held {
            Some(axis) => axis,
            None if moved && raw.x.abs() > raw.y.abs() * 1.2 => ScrollAxis::Horizontal,
            None if moved => ScrollAxis::Vertical,
            None => {
                self.scroll_lock = None;
                return;
            }
        };
        if moved {
            self.scroll_lock = Some((axis, now));
        }
        ctx.input_mut(|input| match axis {
            ScrollAxis::Horizontal => input.smooth_scroll_delta.y = 0.0,
            ScrollAxis::Vertical => input.smooth_scroll_delta.x = 0.0,
        });
    }

    pub fn save_state(&mut self) {
        if self.settings_dirty {
            self.save_settings();
        }
    }

    pub fn shutdown(&mut self) {
        self.save_state();
        self.backend.shutdown();
    }

    /// The media of a message in a loaded chat, for views.
    pub fn media_of(&self, chat: &str, id: &str) -> Option<&Media> {
        self.conversations.get(chat)?.message(id)?.content.media()
    }
}

/// A picture on the clipboard, as (width, height, straight RGBA).
fn clipboard_image() -> Option<(usize, usize, Vec<u8>)> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    if image.width == 0 || image.height == 0 {
        return None;
    }
    Some((image.width, image.height, image.bytes.into_owned()))
}

impl Delivery {
    /// Whether the message is ours and still on its way.
    pub fn in_flight(self) -> bool {
        matches!(self, Delivery::Pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Content;

    fn app() -> App {
        let root = std::env::temp_dir().join(format!("fastsapp-app-{}", std::process::id()));
        App::headless(AppDirs::under(&root), Settings::default()).0
    }

    fn message(chat: &str, id: &str, timestamp: i64) -> Message {
        Message {
            id: id.into(),
            chat: chat.into(),
            sender: chat.into(),
            sender_name: None,
            from_me: false,
            timestamp,
            content: Content::text(id),
            status: Delivery::None,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    #[test]
    fn conversations_merge_pages_without_duplicates() {
        let mut conversation = Conversation::default();
        conversation.merge(vec![message("c", "b", 2), message("c", "c", 3)], false);
        conversation.merge(vec![message("c", "a", 1), message("c", "b", 2)], true);
        let ids: Vec<&str> = conversation
            .messages
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
        conversation.merge(vec![message("c", "c", 3)], false);
        assert_eq!(conversation.messages.len(), 3);
    }

    #[test]
    fn visible_chats_pin_first_and_filter() {
        let mut app = app();
        let mut a = Chat::new("1@s.whatsapp.net".into(), "Ada".into());
        a.last_activity = 10;
        let mut b = Chat::new("2@s.whatsapp.net".into(), "Bob".into());
        b.last_activity = 20;
        let mut c = Chat::new("3@s.whatsapp.net".into(), "Cy".into());
        c.last_activity = 5;
        c.pinned = true;
        let mut d = Chat::new("4@s.whatsapp.net".into(), "Dee".into());
        d.archived = true;
        app.chats = vec![b, a, c, d];
        let names: Vec<&str> = app
            .visible_chats()
            .iter()
            .map(|chat| chat.name.as_str())
            .collect();
        assert_eq!(names, vec!["Cy", "Bob", "Ada"]);
        app.search = "ad".into();
        let names: Vec<&str> = app
            .visible_chats()
            .iter()
            .map(|chat| chat.name.as_str())
            .collect();
        assert_eq!(names, vec!["Ada"]);
    }

    #[test]
    fn opening_a_chat_keeps_drafts_apart() {
        let mut app = app();
        app.chats
            .push(Chat::new("1@s.whatsapp.net".into(), "Ada".into()));
        app.chats
            .push(Chat::new("2@s.whatsapp.net".into(), "Bob".into()));
        app.open_chat("1@s.whatsapp.net".into());
        app.composer = "hello ada".into();
        app.open_chat("2@s.whatsapp.net".into());
        assert_eq!(app.composer, "");
        app.open_chat("1@s.whatsapp.net".into());
        assert_eq!(app.composer, "hello ada");
        assert_eq!(app.settings.last_chat.as_deref(), Some("1@s.whatsapp.net"));
    }

    #[test]
    fn names_fall_back_from_contacts_to_phones() {
        let mut app = app();
        app.contacts.insert(
            "1@s.whatsapp.net".into(),
            Contact {
                id: "1@s.whatsapp.net".into(),
                full_name: Some("Ada".into()),
                push_name: None,
            },
        );
        assert_eq!(app.display_name("1@s.whatsapp.net"), "Ada");
        assert_eq!(
            app.display_name("393331234567@s.whatsapp.net"),
            "+39 333 123 456 7"
        );
        assert_eq!(app.display_name("42@lid"), "Unknown");
        app.contacts.insert(
            "42@lid".into(),
            Contact {
                id: "42@lid".into(),
                full_name: None,
                push_name: Some("Bob".into()),
            },
        );
        assert_eq!(app.display_name("42@lid"), "~Bob");
        app.me = Some("42@lid".into());
        assert_eq!(app.display_name("42@lid"), "You");
    }
}
