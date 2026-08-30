//! The runtime side of the backend: the WhatsApp connection, the archive,
//! attachments, and profile pictures, all on the tokio runtime.
//!
//! The library hands over decrypted messages and a typed event stream; it
//! keeps no chats, so every message goes into the archive here before the
//! interface hears about it. Ids are canonical: a chat behind a privacy id
//! (`@lid`) is filed under its phone number as soon as the two are known to
//! belong together, so receipts, typing, and history all land on one row.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use whatsapp_rust::download::MediaType;
use whatsapp_rust::media::{
    AudioOptions, DocumentOptions, ImageOptions, VideoOptions, audio_message, document_message,
    image_message, video_message,
};
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::prelude::{
    Bot, BotHandle, Client, Jid, MessageBuilderExt, MessageExt, MessageField, SendOptions,
    SqliteStore, wa,
};
use whatsapp_rust::send::RevokeType;
use whatsapp_rust::types::events as wa_events;
use whatsapp_rust::types::message::{MessageInfo, MessageSource};
use whatsapp_rust::types::presence::{ChatPresence, ReceiptType};
use whatsapp_rust::upload::UploadOptions;
use whatsapp_rust::wacore::download::Downloadable;
use whatsapp_rust::wacore::history_sync::{HistorySyncStream, MAX_DECOMPRESSED};
use whatsapp_rust::wacore::store::DevicePropsOverride;
use whatsapp_rust::wacore_binary::jid::JidExt;
use whatsapp_rust::waproto::buffa::Message as _;

use super::{Command, Event, LinkStatus, Waker};
use crate::app::PAGE;
use crate::archive::Archive;
use crate::model::{
    Chat, ChatId, ChatKind, Contact, Content, Delivery, Gif, LinkPreview, Media, MentionRef,
    Message, Quoted, Reaction,
};
use crate::paths::AppDirs;

/// How long after the last history chunk the sync is considered over.
const SYNC_QUIET: Duration = Duration::from_secs(20);
/// A cached profile picture is trusted for this long.
const AVATAR_FRESH: Duration = Duration::from_secs(24 * 60 * 60);
/// How long the phone gets to answer a request for older messages.
const PHONE_PATIENCE: Duration = Duration::from_secs(30);
/// How many older messages to ask the phone for at a time.
const PHONE_BATCH: i32 = 50;
/// `HistorySync.sync_type` for an answer to our own request.
const ON_DEMAND: i32 = 6;
/// The longest side of the little picture sent ahead of an attachment.
const THUMBNAIL_SIDE: u32 = 96;
/// How many unfetched sticker messages the picker fills in at a time.
const STICKER_FETCH_LIMIT: usize = 40;

/// One of the phone's recent stickers, as something the client can fetch.
struct PhoneSticker(wa::StickerMetadata);

impl Downloadable for PhoneSticker {
    fn direct_path(&self) -> Option<&str> {
        self.0.direct_path.as_deref()
    }

    fn media_key(&self) -> Option<&[u8]> {
        self.0.media_key.as_deref()
    }

    fn file_enc_sha256(&self) -> Option<&[u8]> {
        self.0.file_enc_sha256.as_deref()
    }

    fn file_sha256(&self) -> Option<&[u8]> {
        self.0.file_sha256.as_deref()
    }

    fn file_length(&self) -> Option<u64> {
        self.0.file_length
    }

    fn app_info(&self) -> MediaType {
        MediaType::Sticker
    }
}

/// This build's version, as WhatsApp's device properties spell it.
fn app_version() -> wa::device_props::AppVersion {
    let mut parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|part| part.parse::<u32>().ok());
    wa::device_props::AppVersion {
        primary: parts.next().flatten(),
        secondary: parts.next().flatten(),
        tertiary: parts.next().flatten(),
        ..Default::default()
    }
}

/// A sticker's identity across messages and the phone's list: its plain
/// file hash, else the encrypted one, as hex.
fn sticker_hash(sha256: Option<&[u8]>, enc_sha256: Option<&[u8]>) -> Option<String> {
    let bytes = sha256
        .filter(|bytes| !bytes.is_empty())
        .or(enc_sha256.filter(|bytes| !bytes.is_empty()))?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub async fn run(
    dirs: AppDirs,
    events: std::sync::mpsc::Sender<Event>,
    commands: mpsc::UnboundedSender<Command>,
    mut inbox: mpsc::UnboundedReceiver<Command>,
    waker: Waker,
) {
    let archive = match Archive::open(&dirs.archive_db()) {
        Ok(archive) => archive,
        Err(error) => {
            log::error!("could not open the message archive, keeping it in memory: {error}");
            let _ = events.send(Event::Error(format!(
                "The message archive could not be opened; nothing will be kept: {error}"
            )));
            match Archive::in_memory() {
                Ok(archive) => archive,
                Err(error) => {
                    let _ = events.send(Event::Link(LinkStatus::Failed(format!(
                        "SQLite is unusable: {error}"
                    ))));
                    return;
                }
            }
        }
    };
    let (wa_sender, wa_events) = mpsc::unbounded_channel();
    let mut worker = Worker {
        dirs,
        events,
        commands,
        waker,
        archive,
        client: None,
        handle: None,
        wa_sender,
        me_pn: None,
        me_lid: None,
        me_name: None,
        me_about: None,
        lid_to_pn: HashMap::new(),
        contacts: HashMap::new(),
        status: LinkStatus::Starting,
        pairing_phone: None,
        pair_code: None,
        qr: None,
        syncing: false,
        sync_deadline: None,
        group_info_requested: HashSet::new(),
        presence_subscribed: HashSet::new(),
        pending_older: HashMap::new(),
        older_warned: HashSet::new(),
        pending_avatars: HashMap::new(),
        sticker_fetches: HashSet::new(),
        sticker_downloads: HashSet::new(),
    };
    worker.load_state();
    worker.backfill();
    worker.relocate_media();
    worker.start_bot().await;
    let mut wa_events = wa_events;
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        let deadline = worker.sync_deadline;
        tokio::select! {
            command = inbox.recv() => {
                match command {
                    Some(Command::Shutdown) | None => break,
                    Some(command) => worker.handle_command(command).await,
                }
            }
            Some(event) = wa_events.recv() => worker.handle_wa_event(event).await,
            _ = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                worker.sync_deadline = None;
                worker.set_syncing(false);
                worker.emit_chats();
            }
            _ = tick.tick() => {
                worker.expire_older_requests();
                worker.retry_avatars();
            }
        }
    }
    worker.stop_bot().await;
}

struct Worker {
    dirs: AppDirs,
    events: std::sync::mpsc::Sender<Event>,
    commands: mpsc::UnboundedSender<Command>,
    waker: Waker,
    archive: Archive,
    client: Option<Arc<Client>>,
    handle: Option<BotHandle>,
    wa_sender: mpsc::UnboundedSender<Arc<wa_events::Event>>,
    me_pn: Option<String>,
    me_lid: Option<String>,
    me_name: Option<String>,
    me_about: Option<String>,
    /// Privacy id user part to phone number user part.
    lid_to_pn: HashMap<String, String>,
    contacts: HashMap<String, Contact>,
    status: LinkStatus,
    pairing_phone: Option<String>,
    pair_code: Option<String>,
    qr: Option<String>,
    syncing: bool,
    sync_deadline: Option<Instant>,
    group_info_requested: HashSet<String>,
    presence_subscribed: HashSet<String>,
    /// Requests for older messages the phone has not answered yet, by the
    /// chat asked about: when it was asked and what its oldest message was.
    pending_older: HashMap<ChatId, (Instant, super::PageKey)>,
    /// Chats already told once that the phone is not answering.
    older_warned: HashSet<ChatId>,
    /// Pictures asked for while the link was down or that failed, with how
    /// often they were tried; retried once connected.
    pending_avatars: HashMap<(String, bool), u32>,
    /// The phone's recent stickers being fetched for the picker, by hash.
    sticker_fetches: HashSet<String>,
    /// Sticker messages being fetched for the picker, by chat and id.
    sticker_downloads: HashSet<(ChatId, String)>,
}

/// A history chunk, decoded off the worker but not yet filed: ids are as
/// WhatsApp wrote them, canonicalised once the chunk's own mappings are in.
struct ParsedHistory {
    chats: Vec<ParsedChat>,
    push_names: Vec<(String, String)>,
    lids: Vec<(String, String)>,
    /// The phone's recently used stickers, sent along with the history.
    stickers: Vec<wa::StickerMetadata>,
}

struct ParsedChat {
    id: String,
    name: Option<String>,
    unread: u32,
    archived: bool,
    pinned: bool,
    muted_until: Option<i64>,
    last_activity: i64,
    pn_jid: Option<String>,
    lid_jid: Option<String>,
    /// Whether the phone says it holds more than it sent.
    more_on_phone: Option<bool>,
    messages: Vec<ParsedMessage>,
    revoked: Vec<String>,
}

struct ParsedMessage {
    id: String,
    sender: Option<String>,
    from_me: bool,
    push_name: Option<String>,
    timestamp: i64,
    content: Content,
    status: Delivery,
    quoted: Option<Quoted>,
    reactions: Vec<(Option<String>, bool, String)>,
    mentions: Vec<String>,
    forwarded: bool,
    thumbnail: Option<Vec<u8>>,
    raw: Vec<u8>,
}

impl Worker {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        self.waker.wake();
    }

    /// Runs a chat setting's sync to the phone off the worker, when there
    /// is a link; the archive is already updated, so a failure only logs.
    fn tell_phone<F, Fut>(&self, chat: &str, call: F)
    where
        F: FnOnce(Arc<Client>, Jid) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(chat)) else {
            return;
        };
        let chat = chat.to_owned();
        tokio::spawn(async move {
            if let Err(error) = call(client, jid).await {
                log::warn!("the phone was not told about {chat}: {error}");
            }
        });
    }

    fn emit_chats(&self) {
        match self.archive.chats() {
            Ok(mut chats) => {
                for chat in &mut chats {
                    self.polish_chat(chat);
                }
                self.emit(Event::Chats(chats));
            }
            Err(error) => log::warn!("could not list chats: {error}"),
        }
    }

    fn emit_chat(&self, id: &str) {
        if let Ok(Some(mut chat)) = self.archive.chat(id) {
            self.polish_chat(&mut chat);
            self.emit(Event::ChatUpdated(Box::new(chat)));
        }
    }

    /// The preview under a chat's name names people by phone number, so
    /// the interface can put names to them.
    fn polish_chat(&self, chat: &mut Chat) {
        if let Some(last) = chat.last.as_mut() {
            last.summary = self.pn_tokens(&last.summary);
        }
    }

    fn emit_message(&self, chat: &str, id: &str) {
        if let Ok(Some(message)) = self.archive.message(chat, id) {
            self.emit(Event::MessageUpdated(Box::new(message)));
        }
    }

    fn set_status(&mut self, status: LinkStatus) {
        if self.status != status {
            log::info!("link: {status:?}");
            self.status = status.clone();
            self.emit(Event::Link(status));
        }
    }

    fn set_syncing(&mut self, syncing: bool) {
        if self.syncing != syncing {
            self.syncing = syncing;
            self.emit(Event::Syncing(syncing));
        }
    }

    fn unlinked(&self) -> LinkStatus {
        LinkStatus::Unlinked {
            qr: self.qr.clone(),
            pair_code: self.pair_code.clone(),
            pairing_phone: self.pairing_phone.clone(),
        }
    }

    /// Our own id as chats and messages use it.
    fn me(&self) -> String {
        self.me_pn
            .clone()
            .or_else(|| self.me_lid.clone())
            .unwrap_or_else(|| "me".to_owned())
    }

    fn is_me(&self, id: &str) -> bool {
        self.me_pn.as_deref() == Some(id) || self.me_lid.as_deref() == Some(id)
    }

    fn load_state(&mut self) {
        self.me_pn = self.archive.meta("me_pn").ok().flatten();
        self.me_lid = self.archive.meta("me_lid").ok().flatten();
        self.me_name = self.archive.meta("me_name").ok().flatten();
        self.me_about = self.archive.meta("me_about").ok().flatten();
        if let Ok(lids) = self.archive.lids() {
            self.lid_to_pn = lids.into_iter().collect();
        }
        if let Ok(contacts) = self.archive.contacts() {
            self.contacts = contacts
                .into_iter()
                .map(|contact| (contact.id.clone(), contact))
                .collect();
        }
        if let Some(id) = self.me_pn.clone().or_else(|| self.me_lid.clone()) {
            self.emit(Event::Me {
                id,
                name: self.me_name.clone(),
                about: self.me_about.clone(),
            });
        }
        self.emit(Event::Contacts(self.contacts.values().cloned().collect()));
        self.emit_chats();
    }

    /// Re-reads every archived protobuf once after an update that learned
    /// to read more out of it (previews, mentions, thumbnails, the
    /// forwarded mark), so older chats look the same as new ones.
    /// Attachments are filed by absolute path. When the cache moved (the
    /// app's rename did that) or a file was cleared out, a path that no
    /// longer exists is pointed at the file in the current cache, or
    /// forgotten so the attachment is fetched again when wanted.
    fn relocate_media(&mut self) {
        let dir = self.dirs.media_cache_dir();
        let rows = match self.archive.media_paths() {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!("could not list attachments: {error}");
                return;
            }
        };
        let (mut moved, mut forgotten) = (0, 0);
        for (chat, id, path) in rows {
            if path.exists() {
                continue;
            }
            let candidate = path.file_name().map(|name| dir.join(name));
            match candidate.filter(|candidate| candidate.exists()) {
                Some(candidate) => {
                    if self.archive.set_media_path(&chat, &id, &candidate).is_ok() {
                        moved += 1;
                    }
                }
                None => {
                    if self.archive.clear_media_path(&chat, &id).is_ok() {
                        forgotten += 1;
                    }
                }
            }
        }
        if moved + forgotten > 0 {
            log::info!(
                "attachments: {moved} re-pointed to {}, {forgotten} to fetch again",
                dir.display()
            );
        }
    }

    fn backfill(&mut self) {
        const VERSION: &str = "2";
        if self.archive.meta("derived").ok().flatten().as_deref() == Some(VERSION) {
            return;
        }
        let rows = match self.archive.rows_with_raw() {
            Ok(rows) => rows,
            Err(error) => {
                log::warn!("could not read the archive for re-deriving: {error}");
                return;
            }
        };
        let started = Instant::now();
        let mut updated = 0;
        for (chat, id, raw) in rows {
            let Ok(message) = wa::Message::decode_from_slice(&raw) else {
                continue;
            };
            let base = message.get_base_message();
            let Some(mut content) = classify(base) else {
                continue;
            };
            let Ok(Some(existing)) = self.archive.message(&chat, &id) else {
                continue;
            };
            if matches!(existing.content, Content::Revoked) {
                continue;
            }
            if let (Some(new), Some(old)) = (content.media_mut(), existing.content.media()) {
                new.path = old.path.clone();
            }
            let mentions = self.mentions_of(&mentioned_of(base));
            let thumbnail = thumbnail_of(base);
            if self
                .archive
                .set_derived(
                    &chat,
                    &id,
                    &content,
                    &mentions,
                    thumbnail.as_deref(),
                    forwarded_of(base),
                )
                .is_ok()
            {
                updated += 1;
            }
        }
        let _ = self.archive.set_meta("derived", VERSION);
        if updated > 0 {
            log::info!(
                "re-derived {updated} archived messages in {:.1?}",
                started.elapsed()
            );
            self.emit_chats();
        }
    }

    async fn start_bot(&mut self) {
        let path = self.dirs.session_db();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let store = match SqliteStore::new(&path.to_string_lossy()).await {
            Ok(store) => store,
            Err(error) => {
                self.set_status(LinkStatus::Failed(format!(
                    "The device store could not be opened: {error}"
                )));
                return;
            }
        };
        let sender = self.wa_sender.clone();
        let bot = Bot::builder()
            .with_backend(store)
            // What the phone lists under Linked devices: the app's name and
            // version, with the desktop icon. Read at pairing only, so a
            // device linked earlier keeps what it was linked as.
            .with_device_props(
                DevicePropsOverride::new()
                    .with_os("Fastsapp")
                    .with_version(app_version())
                    .with_platform_type(wa::device_props::PlatformType::DESKTOP),
            )
            .on_event(move |event, _client| {
                let sender = sender.clone();
                async move {
                    let _ = sender.send(event);
                }
            })
            .build()
            .await;
        match bot {
            Ok(bot) => {
                let handle = bot.spawn();
                self.client = Some(handle.client());
                self.handle = Some(handle);
                self.set_status(LinkStatus::Connecting);
            }
            Err(error) => self.set_status(LinkStatus::Failed(format!(
                "WhatsApp could not start: {error}"
            ))),
        }
    }

    async fn stop_bot(&mut self) {
        self.client = None;
        if let Some(handle) = self.handle.take()
            && tokio::time::timeout(Duration::from_secs(5), handle.shutdown())
                .await
                .is_err()
        {
            log::warn!("the WhatsApp connection did not stop in time");
        }
    }

    // --- ids -------------------------------------------------------------

    fn learn_lid(&mut self, lid: &str, pn: &str) {
        if lid.is_empty() || pn.is_empty() {
            return;
        }
        if self.lid_to_pn.get(lid).is_some_and(|known| known == pn) {
            return;
        }
        self.lid_to_pn.insert(lid.to_owned(), pn.to_owned());
        if let Err(error) = self.archive.put_lid(lid, pn) {
            log::warn!("could not remember an id mapping: {error}");
        }
    }

    fn learn_pair(&mut self, a: &Jid, b: &Jid) {
        if a.is_lid() && b.is_pn() {
            self.learn_lid(a.user_base(), b.user_base());
        } else if a.is_pn() && b.is_lid() {
            self.learn_lid(b.user_base(), a.user_base());
        }
    }

    fn learn_source(&mut self, source: &MessageSource) {
        if let Some(alt) = &source.sender_alt {
            let sender = source.sender.clone();
            self.learn_pair(&sender, alt);
        }
        if let Some(alt) = &source.recipient_alt {
            let chat = source
                .recipient
                .clone()
                .unwrap_or_else(|| source.chat.clone());
            self.learn_pair(&chat, alt);
        }
    }

    /// The archive's id for a JID: the phone-number form when the JID is a
    /// privacy id we can resolve.
    fn canonical(&self, jid: &Jid) -> String {
        if jid.is_lid()
            && let Some(pn) = self.lid_to_pn.get(jid.user_base())
        {
            let pn = format!("{pn}@s.whatsapp.net");
            return if self.is_me(&pn) { self.me() } else { pn };
        }
        let id = jid.to_non_ad_string();
        if self.is_me(&id) {
            return self.me();
        }
        id
    }

    fn canonical_str(&self, id: &str) -> String {
        match id.parse::<Jid>() {
            Ok(jid) => self.canonical(&jid),
            Err(_) => id.to_owned(),
        }
    }

    fn jid_of(id: &str) -> Option<Jid> {
        id.parse().ok()
    }

    // --- names -----------------------------------------------------------

    fn contact_name(&self, id: &str) -> Option<String> {
        self.contacts.get(id).and_then(Contact::label)
    }

    /// What to call someone in a quote or a mention: "You", the address
    /// book, the name they chose, or the number.
    fn name_for(&self, id: &str) -> Option<String> {
        if self.is_me(id) || id == self.me() {
            return Some("You".to_owned());
        }
        if let Some(name) = self.contact_name(id) {
            return Some(name);
        }
        crate::model::phone_of(id).map(crate::util::phone)
    }

    /// The best name for a chat right now.
    fn chat_name(&self, id: &str, push_name: Option<&str>) -> String {
        if id == self.me() {
            return "You".to_owned();
        }
        if let Some(name) = self
            .contacts
            .get(id)
            .and_then(|contact| contact.full_name.clone())
            .filter(|name| !name.is_empty())
        {
            return name;
        }
        if let Some(digits) = crate::model::phone_of(id) {
            return crate::util::phone(digits);
        }
        if let Some(name) = push_name
            .filter(|name| !name.is_empty())
            .or_else(|| self.contacts.get(id)?.push_name.as_deref())
        {
            return format!("~{name}");
        }
        fallback_name(id)
    }

    fn remember_push_name(&mut self, id: &str, push_name: &str) {
        if push_name.is_empty() || id == self.me() {
            return;
        }
        let contact = self
            .contacts
            .entry(id.to_owned())
            .or_insert_with(|| Contact {
                id: id.to_owned(),
                full_name: None,
                push_name: None,
            });
        if contact.push_name.as_deref() == Some(push_name) {
            return;
        }
        contact.push_name = Some(push_name.to_owned());
        let contact = contact.clone();
        if let Err(error) = self.archive.upsert_contact(&contact) {
            log::warn!("could not save a contact: {error}");
        }
        self.emit(Event::Contacts(vec![contact]));
        self.refresh_chat_name(id);
    }

    /// Renames a chat whose stored name was a fallback, now that better is
    /// known.
    fn refresh_chat_name(&mut self, id: &str) {
        let Ok(Some(chat)) = self.archive.chat(id) else {
            return;
        };
        if chat.kind == ChatKind::Group {
            return;
        }
        let name = self.chat_name(id, None);
        if name != chat.name {
            let _ = self.archive.rename_chat(id, &name);
            self.emit_chat(id);
        }
    }

    fn ensure_chat(&mut self, id: &str, push_name: Option<&str>) {
        match self.archive.chat(id) {
            Ok(Some(chat)) => {
                if chat.kind != ChatKind::Group {
                    let name = self.chat_name(id, push_name);
                    if name != chat.name {
                        let _ = self.archive.rename_chat(id, &name);
                    }
                }
            }
            Ok(None) => {
                let name = self.chat_name(id, push_name);
                if let Err(error) = self.archive.ensure_chat(id, &name) {
                    log::warn!("could not create chat {id}: {error}");
                }
            }
            Err(error) => log::warn!("could not read chat {id}: {error}"),
        }
        if ChatKind::from_id(id) == ChatKind::Group {
            self.request_group_info(id, false);
        }
    }

    /// Asks WhatsApp about a group once, or again when `force`.
    fn request_group_info(&mut self, id: &str, force: bool) {
        if force {
            self.group_info_requested.remove(id);
        } else {
            let known = self.archive.chat(id).ok().flatten().is_some_and(|chat| {
                chat.name != fallback_name(id) && !chat.participants.is_empty()
            });
            if known {
                return;
            }
        }
        if !self.group_info_requested.insert(id.to_owned()) {
            return;
        }
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(id)) else {
            self.group_info_requested.remove(id);
            return;
        };
        let commands = self.commands.clone();
        let chat = id.to_owned();
        let me: Vec<String> = [self.me_pn.clone(), self.me_lid.clone()]
            .into_iter()
            .flatten()
            .collect();
        let lids = self.lid_to_pn.clone();
        tokio::spawn(async move {
            match client.groups().get_metadata(&jid).await {
                Ok(metadata) => {
                    let canonical = |jid: &Jid| -> String {
                        if jid.is_lid()
                            && let Some(pn) = lids.get(jid.user_base())
                        {
                            return format!("{pn}@s.whatsapp.net");
                        }
                        jid.to_non_ad_string()
                    };
                    let mut participants = Vec::new();
                    let mut admin = false;
                    for participant in &metadata.participants {
                        let id = participant
                            .phone_number
                            .as_ref()
                            .map(canonical)
                            .unwrap_or_else(|| canonical(&participant.jid));
                        let mine = me.contains(&id)
                            || participant
                                .lid
                                .as_ref()
                                .is_some_and(|lid| me.contains(&lid.to_non_ad_string()))
                            || me.contains(&participant.jid.to_non_ad_string());
                        if mine && participant.is_admin() {
                            admin = true;
                        }
                        participants.push(id);
                    }
                    let _ = commands.send(Command::GroupInfo {
                        chat,
                        name: (!metadata.subject.is_empty()).then(|| metadata.subject.clone()),
                        participants,
                        read_only: metadata.is_announcement && !admin,
                    });
                }
                Err(error) => {
                    log::debug!("no metadata for {chat}: {error}");
                    let _ = commands.send(Command::GroupInfoFailed { chat });
                }
            }
        });
    }

    // --- WhatsApp events -------------------------------------------------

    async fn handle_wa_event(&mut self, event: Arc<wa_events::Event>) {
        use wa_events::Event as E;
        match &*event {
            E::PairingQrCode(qr) => {
                self.qr = Some(qr.code.clone());
                let status = self.unlinked();
                self.set_status(status);
            }
            E::PairingCode(code) => {
                self.pair_code = Some(code.code.clone());
                let status = self.unlinked();
                self.set_status(status);
            }
            E::PairingCodeError(error) => {
                self.pair_code = None;
                self.pairing_phone = None;
                self.emit(Event::Error(format!(
                    "Pairing by phone failed: {}",
                    error.error
                )));
                let status = self.unlinked();
                self.set_status(status);
            }
            E::PairingQrCodesExhausted(exhausted) => {
                self.qr = None;
                let status = self.unlinked();
                self.set_status(status);
                if exhausted.disconnected
                    && let Some(client) = self.client.clone()
                {
                    tokio::spawn(async move { client.reconnect_immediately().await });
                }
            }
            E::PairSuccess(pair) => {
                self.qr = None;
                self.pair_code = None;
                self.pairing_phone = None;
                self.remember_identity(Some(pair.id.clone()), Some(pair.lid.clone()), None);
                self.set_status(LinkStatus::Connecting);
            }
            E::Connected(_) => {
                let (pn, lid, name) = match &self.client {
                    Some(client) => (client.pn(), client.lid(), Some(client.push_name())),
                    None => (None, None, None),
                };
                self.remember_identity(pn, lid, name);
                self.set_status(LinkStatus::Connected);
                self.retry_avatars();
                if let Some(client) = self.client.clone() {
                    let me = self.me_pn.clone().and_then(|pn| Self::jid_of(&pn));
                    let commands = self.commands.clone();
                    tokio::spawn(async move {
                        if let Err(error) = client.presence().set_available().await {
                            log::debug!("presence not announced: {error}");
                        }
                        if let Some(me) = me {
                            match client
                                .contacts()
                                .get_user_info(std::slice::from_ref(&me))
                                .await
                            {
                                Ok(info) => {
                                    let about = info
                                        .get(&me)
                                        .and_then(|info| info.status.clone())
                                        .filter(|about| !about.is_empty());
                                    let _ = commands.send(Command::MeInfo { about });
                                }
                                Err(error) => log::debug!("own info not fetched: {error}"),
                            }
                        }
                    });
                }
            }
            E::Disconnected(disconnected) => {
                if matches!(self.status, LinkStatus::Connected | LinkStatus::Connecting) {
                    self.set_status(LinkStatus::Disconnected {
                        reason: disconnected.reason.to_string(),
                    });
                }
            }
            E::LoggedOut(_) => self.on_logged_out().await,
            E::ConnectFailure(failure) => {
                if !failure.reason.is_logged_out() {
                    let detail = failure
                        .message
                        .as_ref()
                        .map(|message| format!(": {message}"))
                        .unwrap_or_default();
                    self.emit(Event::Error(format!(
                        "Connection failed ({:?}){detail}",
                        failure.reason
                    )));
                }
            }
            E::StreamReplaced(_) => {
                self.emit(Event::Error(
                    "Another WhatsApp Web session replaced this one".to_owned(),
                ));
            }
            E::TemporaryBan(ban) => {
                self.set_status(LinkStatus::Failed(format!(
                    "WhatsApp has temporarily blocked this account ({:?})",
                    ban.code
                )));
            }
            E::ClientOutdated(_) => {
                self.set_status(LinkStatus::Failed(
                    "WhatsApp rejected this client as outdated; update Fastsapp".to_owned(),
                ));
            }
            E::Messages(batch) => {
                for inbound in batch.messages.iter() {
                    self.ingest(&inbound.message, &inbound.info);
                }
            }
            E::UndecryptableMessage(undecryptable) => {
                self.ingest_undecryptable(&undecryptable.info);
            }
            E::Receipt(receipt) => self.on_receipt(receipt),
            E::ChatPresence(presence) => {
                self.learn_source(&presence.source);
                self.emit(Event::Typing {
                    chat: self.canonical(&presence.source.chat),
                    sender: self.canonical(&presence.source.sender),
                    composing: matches!(presence.state, ChatPresence::Composing),
                });
            }
            E::Presence(presence) => {
                self.emit(Event::Presence {
                    id: self.canonical(&presence.from),
                    online: !presence.unavailable,
                    last_seen: presence.last_seen.map(|when| when.timestamp()),
                });
            }
            E::ContactUpdate(update) => self.on_contact_update(update),
            E::GroupUpdate(update) => {
                let chat = self.canonical(&update.group_jid);
                self.request_group_info(&chat, true);
            }
            E::ArchiveUpdate(update) => {
                let chat = self.canonical(&update.jid);
                let _ = self
                    .archive
                    .set_archived(&chat, update.action.archived.unwrap_or(false));
                self.emit_chat(&chat);
            }
            E::PinUpdate(update) => {
                let chat = self.canonical(&update.jid);
                let _ = self
                    .archive
                    .set_pinned(&chat, update.action.pinned.unwrap_or(false));
                self.emit_chat(&chat);
            }
            E::MuteUpdate(update) => {
                let chat = self.canonical(&update.jid);
                let until = if update.action.muted.unwrap_or(false) {
                    Some(seconds(update.action.mute_end_timestamp.unwrap_or(0)))
                } else {
                    None
                };
                let _ = self.archive.set_muted(&chat, until);
                self.emit_chat(&chat);
            }
            E::MarkChatAsReadUpdate(update) => {
                let chat = self.canonical(&update.jid);
                if update.action.read.unwrap_or(true) {
                    let _ = self.archive.mark_read(&chat);
                } else {
                    let _ = self.archive.set_unread(&chat, 1);
                }
                self.emit_chat(&chat);
            }
            E::HistorySync(lazy) => self.on_history_sync(lazy).await,
            E::PictureUpdate(update) => {
                let id = self.canonical(&update.jid);
                let _ = std::fs::remove_file(self.avatar_file(&id, false));
                let _ = std::fs::remove_file(self.avatar_file(&id, true));
                if update.removed {
                    self.emit(Event::Avatar {
                        id: id.clone(),
                        full: false,
                        path: None,
                    });
                    self.emit(Event::Avatar {
                        id,
                        full: true,
                        path: None,
                    });
                } else {
                    self.fetch_avatar(id.clone(), false);
                    self.fetch_avatar(id, true);
                }
            }
            E::SelfPushNameUpdated(update) => {
                self.me_name = Some(update.new_name.clone());
                let _ = self.archive.set_meta("me_name", &update.new_name);
                self.emit(Event::Me {
                    id: self.me(),
                    name: self.me_name.clone(),
                    about: self.me_about.clone(),
                });
            }
            E::OfflineSyncCompleted(_) => self.emit_chats(),
            _ => {}
        }
    }

    fn remember_identity(&mut self, pn: Option<Jid>, lid: Option<Jid>, name: Option<String>) {
        if let Some(pn) = pn {
            let pn = pn.to_non_ad_string();
            let _ = self.archive.set_meta("me_pn", &pn);
            self.me_pn = Some(pn);
        }
        if let Some(lid) = lid {
            let lid = lid.to_non_ad_string();
            let _ = self.archive.set_meta("me_lid", &lid);
            self.me_lid = Some(lid);
        }
        if let (Some(pn), Some(lid)) = (self.me_pn.clone(), self.me_lid.clone())
            && let (Some(pn), Some(lid)) = (Self::jid_of(&pn), Self::jid_of(&lid))
        {
            self.learn_pair(&lid, &pn);
        }
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            let _ = self.archive.set_meta("me_name", &name);
            self.me_name = Some(name);
        }
        self.emit(Event::Me {
            id: self.me(),
            name: self.me_name.clone(),
            about: self.me_about.clone(),
        });
    }

    async fn on_logged_out(&mut self) {
        self.stop_bot().await;
        if let Err(error) = self.archive.clear() {
            log::warn!("could not clear the archive: {error}");
        }
        self.lid_to_pn.clear();
        self.contacts.clear();
        self.group_info_requested.clear();
        self.presence_subscribed.clear();
        self.pending_older.clear();
        self.pending_avatars.clear();
        self.me_pn = None;
        self.me_lid = None;
        self.me_name = None;
        self.me_about = None;
        self.qr = None;
        self.pair_code = None;
        self.pairing_phone = None;
        self.set_syncing(false);
        let session = self.dirs.session_db();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut path = session.clone().into_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(self.dirs.avatar_cache_dir());
        let _ = std::fs::remove_dir_all(self.dirs.media_cache_dir());
        self.emit(Event::Chats(Vec::new()));
        self.set_status(LinkStatus::LoggedOut);
        // A fresh store, so the next connection asks the phone to link again.
        self.start_bot().await;
    }

    fn on_contact_update(&mut self, update: &wa_events::ContactUpdate) {
        if let (Some(lid), Some(pn)) = (&update.action.lid_jid, &update.action.pn_jid)
            && let (Some(lid), Some(pn)) = (Self::jid_of(lid), Self::jid_of(pn))
        {
            self.learn_pair(&lid, &pn);
        }
        let id = self.canonical(&update.jid);
        let name = update
            .action
            .full_name
            .clone()
            .or_else(|| update.action.first_name.clone())
            .filter(|name| !name.is_empty());
        let contact = self.contacts.entry(id.clone()).or_insert_with(|| Contact {
            id: id.clone(),
            full_name: None,
            push_name: None,
        });
        if contact.full_name == name {
            return;
        }
        contact.full_name = name;
        let contact = contact.clone();
        if let Err(error) = self.archive.upsert_contact(&contact) {
            log::warn!("could not save a contact: {error}");
        }
        self.emit(Event::Contacts(vec![contact]));
        self.refresh_chat_name(&id);
    }

    fn on_receipt(&mut self, receipt: &wa_events::Receipt) {
        self.learn_source(&receipt.source);
        let chat = self.canonical(&receipt.source.chat);
        log::debug!(
            "receipt {:?} from {} (chat {chat}, from me: {}, offline: {}) for {:?}",
            receipt.r#type,
            receipt.source.sender,
            receipt.source.is_from_me,
            receipt.offline,
            receipt.message_ids
        );
        let status = match receipt.r#type {
            ReceiptType::Delivered => Delivery::Delivered,
            // Delivered to a device that is inactive: the message reached
            // it, which is what the second tick says.
            ReceiptType::Inactive => Delivery::Delivered,
            ReceiptType::Read => Delivery::Read,
            ReceiptType::Played => Delivery::Played,
            ReceiptType::ReadSelf | ReceiptType::PlayedSelf => {
                let _ = self.archive.mark_read(&chat);
                self.emit_chat(&chat);
                return;
            }
            // "Delivered to one of our own devices". In the chat with
            // ourselves that device is the recipient, and the phone shows
            // the message read; anywhere else it says nothing about the
            // peer.
            ReceiptType::Sender if chat == self.me() => Delivery::Read,
            _ => return,
        };
        let mut newest = 0;
        let mut changed = 0;
        for id in &receipt.message_ids {
            match self.archive.set_status(&chat, id, status) {
                Ok(true) => {
                    changed += 1;
                    self.emit_message(&chat, id);
                }
                Ok(false) => {}
                Err(error) => log::warn!("could not file a receipt for {id}: {error}"),
            }
            if let Ok(Some(message)) = self.archive.message(&chat, id) {
                newest = newest.max(message.timestamp);
            }
        }
        log::debug!(
            "receipt moved {changed} of {} messages in {chat} to {status:?}",
            receipt.message_ids.len()
        );
        // A read receipt covers everything before it too.
        if status >= Delivery::Read
            && newest > 0
            && let Ok(ids) = self.archive.advance_statuses(&chat, newest, status)
        {
            for id in ids {
                self.emit_message(&chat, &id);
            }
        }
        self.emit_chat(&chat);
    }

    /// The people a message names, as WhatsApp wrote them and as the
    /// archive knows them.
    fn mentions_of(&self, raw: &[String]) -> Vec<MentionRef> {
        raw.iter()
            .filter_map(|jid| {
                let user = jid.split('@').next()?.to_owned();
                if user.is_empty() {
                    return None;
                }
                Some(MentionRef {
                    user,
                    id: self.canonical_str(jid),
                })
            })
            .collect()
    }

    fn ingest(&mut self, message: &Arc<wa::Message>, info: &MessageInfo) {
        self.learn_source(&info.source);
        if info.source.chat.is_status_broadcast() {
            return;
        }
        let chat = self.canonical(&info.source.chat);
        let from_me = info.source.is_from_me;
        let sender = if from_me {
            self.me()
        } else {
            self.canonical(&info.source.sender)
        };
        let push_name = (!info.push_name.is_empty()).then(|| info.push_name.clone());
        let base = message.get_base_message();

        if let Some(protocol) = base.protocol_message.as_option() {
            let Some(target) = protocol.key.as_option().and_then(|key| key.id.clone()) else {
                return;
            };
            use wa::message::protocol_message::Type;
            match protocol.r#type {
                Some(Type::REVOKE) => {
                    if let Ok(true) =
                        self.archive
                            .set_content(&chat, &target, &Content::Revoked, false)
                    {
                        self.emit_message(&chat, &target);
                        self.emit_chat(&chat);
                    }
                }
                Some(Type::MESSAGE_EDIT) => {
                    if let Some(edited) = protocol.edited_message.as_option()
                        && let Some(mut content) = classify(edited.get_base_message())
                    {
                        // An edited caption keeps the file already fetched.
                        if let Ok(Some(existing)) = self.archive.message(&chat, &target)
                            && let (Some(new), Some(old)) =
                                (content.media_mut(), existing.content.media())
                        {
                            new.path = old.path.clone();
                        }
                        if let Ok(true) = self.archive.set_content(&chat, &target, &content, true) {
                            self.emit_message(&chat, &target);
                            self.emit_chat(&chat);
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(reaction) = base.reaction_message.as_option() {
            let Some(target) = reaction.key.as_option().and_then(|key| key.id.clone()) else {
                return;
            };
            let emoji = reaction.text.clone().unwrap_or_default();
            if let Ok(Some(updated)) = self
                .archive
                .set_reaction(&chat, &target, &sender, from_me, &emoji)
            {
                self.emit(Event::MessageUpdated(Box::new(updated)));
            }
            return;
        }
        let Some(content) = classify(base) else {
            return;
        };
        let quoted = self.quoted_of(base);
        let mentions = self.mentions_of(&mentioned_of(base));
        let row = Message {
            id: info.id.clone(),
            chat: chat.clone(),
            sender,
            sender_name: if from_me { None } else { push_name.clone() },
            from_me,
            timestamp: info.timestamp.timestamp(),
            content,
            status: if from_me {
                Delivery::Sent
            } else {
                Delivery::None
            },
            quoted,
            reactions: Vec::new(),
            edited: false,
            mentions,
            forwarded: forwarded_of(base),
            thumbnail: thumbnail_of(base),
        };
        self.store_message(row, Some(message.encode_to_vec()), push_name.as_deref());
    }

    fn ingest_undecryptable(&mut self, info: &MessageInfo) {
        self.learn_source(&info.source);
        if info.source.chat.is_status_broadcast() || info.source.is_from_me {
            return;
        }
        let chat = self.canonical(&info.source.chat);
        if self
            .archive
            .message(&chat, &info.id)
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        let push_name = (!info.push_name.is_empty()).then(|| info.push_name.clone());
        let row = Message {
            id: info.id.clone(),
            chat,
            sender: self.canonical(&info.source.sender),
            sender_name: push_name.clone(),
            from_me: false,
            timestamp: info.timestamp.timestamp(),
            content: Content::Unsupported {
                what: "waiting for this message; open WhatsApp on your phone".to_owned(),
            },
            status: Delivery::None,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        };
        self.store_message(row, None, push_name.as_deref());
    }

    /// Files a message and tells the interface about the chat and the row.
    fn store_message(&mut self, message: Message, raw: Option<Vec<u8>>, push_name: Option<&str>) {
        let chat = message.chat.clone();
        self.ensure_chat(&chat, if message.from_me { None } else { push_name });
        if let Some(push_name) = push_name
            && !message.from_me
        {
            let sender = message.sender.clone();
            self.remember_push_name(&sender, push_name);
        }
        let is_new = self
            .archive
            .message(&chat, &message.id)
            .ok()
            .flatten()
            .is_none();
        if let Err(error) = self.archive.insert_message(&message, raw.as_deref()) {
            log::warn!("could not store a message: {error}");
            return;
        }
        if is_new && !message.from_me {
            let _ = self.archive.bump_unread(&chat);
        }
        let stored = self
            .archive
            .message(&chat, &message.id)
            .ok()
            .flatten()
            .unwrap_or(message);
        // Live and from someone else: the desktop may want to say so.
        // History being replayed is not news.
        let incoming = (is_new && !stored.from_me && !self.syncing).then(|| stored.clone());
        self.emit(Event::Messages {
            chat: chat.clone(),
            messages: vec![stored],
            older: false,
            complete: false,
        });
        self.emit_chat(&chat);
        if let Some(message) = incoming {
            self.emit(Event::Incoming {
                chat,
                message: Box::new(message),
            });
        }
    }

    fn quoted_of(&self, base: &wa::Message) -> Option<Quoted> {
        let context = context_of(base)?;
        let id = context.stanza_id.clone().filter(|id| !id.is_empty())?;
        let sender = context
            .participant
            .as_deref()
            .map(|participant| self.canonical_str(participant))
            .unwrap_or_default();
        let (summary, listed) = context
            .quoted_message
            .as_option()
            .map(|quoted| {
                let base = quoted.get_base_message();
                (
                    classify(base)
                        .map(|content| content.summary())
                        .unwrap_or_default(),
                    self.mentions_of(&mentioned_of(base)),
                )
            })
            .unwrap_or_default();
        // A quoted copy often comes without its mention list; the `@user`
        // tokens in the text are the next best thing.
        let summary = self.pn_tokens(&summary);
        let mentions = if listed.is_empty() {
            self.mention_tokens(&summary)
        } else {
            listed
        };
        Some(Quoted {
            sender_name: self.name_for(&sender),
            id,
            sender,
            summary,
            mentions,
        })
    }

    /// `@user` tokens in a text with privacy ids replaced by the phone
    /// numbers the archive names people by, where known.
    fn pn_tokens(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find('@') {
            out.push_str(&rest[..at]);
            out.push('@');
            let after = &rest[at + 1..];
            let digits = after
                .char_indices()
                .find(|(_, c)| !c.is_ascii_digit())
                .map_or(after.len(), |(index, _)| index);
            match self.lid_to_pn.get(&after[..digits]) {
                Some(pn) if digits > 0 => {
                    out.push_str(pn);
                    rest = &after[digits..];
                }
                _ => rest = after,
            }
        }
        out.push_str(rest);
        out
    }

    /// The people `@user` tokens in a text stand for, by the archive's
    /// ids, for text that came without a mention list.
    fn mention_tokens(&self, text: &str) -> Vec<MentionRef> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find('@') {
            let after = &rest[at + 1..];
            let digits = after
                .char_indices()
                .find(|(_, c)| !c.is_ascii_digit())
                .map_or(after.len(), |(index, _)| index);
            let user = &after[..digits];
            if digits >= 5 {
                let id = match self.lid_to_pn.get(user) {
                    Some(pn) => format!("{pn}@s.whatsapp.net"),
                    None => format!("{user}@s.whatsapp.net"),
                };
                let id = self.canonical_str(&id);
                if !found.iter().any(|known: &MentionRef| known.user == user) {
                    found.push(MentionRef {
                        user: user.to_owned(),
                        id,
                    });
                }
            }
            rest = after;
        }
        found
    }

    // --- history ---------------------------------------------------------

    async fn on_history_sync(&mut self, lazy: &wa_events::LazyHistorySync) {
        let on_demand =
            lazy.sync_type() == ON_DEMAND || lazy.peer_data_request_session_id().is_some();
        if !on_demand {
            self.sync_deadline = Some(Instant::now() + SYNC_QUIET);
            self.set_syncing(true);
            if let Some(progress) = lazy.progress() {
                self.emit(Event::SyncProgress(progress.min(100)));
            }
        }
        let compressed = lazy.compressed_bytes().clone();
        let parsed = tokio::task::spawn_blocking(move || parse_history(&compressed)).await;
        match parsed {
            Ok(Ok(parsed)) => {
                let filed = self.apply_history(parsed, !on_demand);
                if on_demand {
                    self.answer_older(filed);
                }
            }
            Ok(Err(error)) => {
                log::warn!("a history chunk could not be read: {error}");
                self.emit(Event::Error(format!(
                    "Part of the history could not be read: {error}"
                )));
            }
            Err(error) => log::warn!("history parsing panicked: {error}"),
        }
        if !on_demand && lazy.progress().is_some_and(|progress| progress >= 100) {
            self.sync_deadline = Some(Instant::now() + Duration::from_secs(3));
        }
        self.emit_chats();
    }

    /// Files a decoded chunk. `metadata` says whether the chunk speaks for
    /// the chats' state (unread, archived…) or only carries messages, as an
    /// answer to a request does. Returns, per chat, how many messages came
    /// and whether the phone holds more.
    fn apply_history(
        &mut self,
        parsed: ParsedHistory,
        metadata: bool,
    ) -> Vec<(ChatId, usize, Option<bool>)> {
        for (lid, pn) in &parsed.lids {
            if let (Some(lid), Some(pn)) = (Self::jid_of(lid), Self::jid_of(pn)) {
                self.learn_pair(&lid, &pn);
            }
        }
        if !parsed.stickers.is_empty() {
            log::info!(
                "the phone listed {} recently used stickers",
                parsed.stickers.len()
            );
        }
        for sticker in &parsed.stickers {
            let Some(hash) = sticker_hash(
                sticker.file_sha256.as_deref(),
                sticker.file_enc_sha256.as_deref(),
            ) else {
                continue;
            };
            if let Err(error) = self.archive.upsert_phone_sticker(
                &hash,
                &sticker.encode_to_vec(),
                seconds(sticker.last_sticker_sent_ts.unwrap_or(0)),
                sticker.weight.unwrap_or(0.0),
            ) {
                log::warn!("could not store sticker {hash}: {error}");
            }
        }
        for chat in &parsed.chats {
            if let (Some(lid), Some(pn)) = (&chat.lid_jid, &chat.pn_jid)
                && let (Some(lid), Some(pn)) = (Self::jid_of(lid), Self::jid_of(pn))
            {
                self.learn_pair(&lid, &pn);
            }
        }
        let mut filed = Vec::new();
        for (id, name) in &parsed.push_names {
            let id = self.canonical_str(id);
            self.remember_push_name(&id, name);
        }
        for chat in parsed.chats {
            let id = self.canonical_str(&chat.id);
            if id.ends_with("@broadcast") {
                continue;
            }
            let existing = self.archive.chat(&id).ok().flatten();
            if metadata || existing.is_none() {
                let name = match chat.name.filter(|name| !name.is_empty()) {
                    Some(name) if ChatKind::from_id(&id) == ChatKind::Group => name,
                    Some(name) => {
                        // A direct chat's conversation name is the address
                        // book name when the phone has one.
                        let contact = self.contacts.entry(id.clone()).or_insert_with(|| Contact {
                            id: id.clone(),
                            full_name: None,
                            push_name: None,
                        });
                        if contact.full_name.is_none()
                            && !name
                                .chars()
                                .all(|c| c.is_ascii_digit() || c == '+' || c == ' ')
                        {
                            contact.full_name = Some(name.clone());
                            let contact = contact.clone();
                            let _ = self.archive.upsert_contact(&contact);
                            self.emit(Event::Contacts(vec![contact]));
                        }
                        self.chat_name(&id, None)
                    }
                    None => self.chat_name(&id, None),
                };
                let mut row = Chat::new(id.clone(), name);
                row.last_activity = chat.last_activity;
                row.unread = existing
                    .as_ref()
                    .map_or(chat.unread, |existing| existing.unread.max(chat.unread));
                row.archived = chat.archived;
                row.pinned = chat.pinned;
                row.muted_until = chat.muted_until;
                if let Err(error) = self.archive.upsert_chat(&row) {
                    log::warn!("could not store chat {id}: {error}");
                    continue;
                }
                if row.unread != existing.as_ref().map_or(0, |existing| existing.unread) {
                    let _ = self.archive.set_unread(&id, row.unread);
                }
            }
            if ChatKind::from_id(&id) == ChatKind::Group {
                self.request_group_info(&id, false);
            }
            let count = chat.messages.len();
            for message in chat.messages {
                let sender = if message.from_me {
                    self.me()
                } else {
                    message
                        .sender
                        .as_deref()
                        .map(|sender| self.canonical_str(sender))
                        .unwrap_or_else(|| id.clone())
                };
                if let Some(push_name) = message.push_name.as_deref()
                    && !message.from_me
                {
                    self.remember_push_name(&sender, push_name);
                }
                let reactions = message
                    .reactions
                    .into_iter()
                    .map(|(who, from_me, emoji)| Reaction {
                        sender: if from_me {
                            self.me()
                        } else {
                            who.as_deref()
                                .map(|who| self.canonical_str(who))
                                .unwrap_or_else(|| id.clone())
                        },
                        from_me,
                        emoji,
                    })
                    .collect();
                let quoted = message.quoted.map(|quoted| {
                    let sender = self.canonical_str(&quoted.sender);
                    Quoted {
                        sender_name: self.name_for(&sender),
                        sender,
                        ..quoted
                    }
                });
                let mentions = self.mentions_of(&message.mentions);
                let row = Message {
                    id: message.id,
                    chat: id.clone(),
                    sender,
                    sender_name: if message.from_me {
                        None
                    } else {
                        message.push_name
                    },
                    from_me: message.from_me,
                    timestamp: message.timestamp,
                    content: message.content,
                    status: message.status,
                    quoted,
                    reactions,
                    edited: false,
                    mentions,
                    forwarded: message.forwarded,
                    thumbnail: message.thumbnail,
                };
                if let Err(error) = self.archive.insert_message(&row, Some(&message.raw)) {
                    log::warn!("could not store a history message: {error}");
                }
            }
            for revoked in chat.revoked {
                let _ = self
                    .archive
                    .set_content(&id, &revoked, &Content::Revoked, false);
            }
            filed.push((id, count, chat.more_on_phone));
        }
        for (id, _, _) in &filed {
            self.emit_chat(id);
        }
        filed
    }

    /// Delivers what the phone sent for a request for older messages.
    fn answer_older(&mut self, filed: Vec<(ChatId, usize, Option<bool>)>) {
        for (chat, count, more_on_phone) in filed {
            let more = count > 0 && more_on_phone != Some(false);
            let Some((_, (before_time, before_id))) = self.pending_older.remove(&chat) else {
                // An answer nobody waits for any more (it came late, or the
                // request was given up on): what it brought is in the
                // archive, and the app pages the archive again on this.
                self.emit(Event::OlderFetched { chat, more });
                continue;
            };
            match self
                .archive
                .messages(&chat, Some((before_time, &before_id)), 500)
            {
                Ok(mut messages) => {
                    for message in &mut messages {
                        self.polish(message);
                    }
                    self.emit(Event::Messages {
                        chat: chat.clone(),
                        messages,
                        older: true,
                        complete: false,
                    })
                }
                Err(error) => log::warn!("could not read older messages: {error}"),
            }
            self.emit(Event::OlderFetched { chat, more });
        }
    }

    /// Gives up on requests the phone has not answered.
    fn expire_older_requests(&mut self) {
        let expired: Vec<ChatId> = self
            .pending_older
            .iter()
            .filter(|(_, (asked, _))| asked.elapsed() > PHONE_PATIENCE)
            .map(|(chat, _)| chat.clone())
            .collect();
        for chat in expired {
            self.pending_older.remove(&chat);
            self.emit(Event::OlderFetched {
                chat: chat.clone(),
                more: true,
            });
            // Once per chat: the app backs off on its own, the reader does
            // not need telling every time.
            if self.older_warned.insert(chat) {
                self.emit(Event::Error(
                    "Your phone did not send older messages; is it online?".to_owned(),
                ));
            }
        }
    }

    fn fetch_older(&mut self, chat: ChatId) {
        if self.pending_older.contains_key(&chat) {
            return;
        }
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            // Offline: nothing to say, the banner says it; the app asks
            // again once connected.
            self.emit(Event::OlderFetched { chat, more: true });
            return;
        };
        let Ok(Some(oldest)) = self.archive.oldest(&chat) else {
            self.emit(Event::OlderFetched { chat, more: false });
            return;
        };
        self.pending_older.insert(
            chat.clone(),
            (Instant::now(), (oldest.timestamp, oldest.id.clone())),
        );
        let commands = self.commands.clone();
        tokio::spawn(async move {
            if let Err(error) = client
                .fetch_message_history(
                    &jid,
                    &oldest.id,
                    oldest.from_me,
                    oldest.timestamp * 1000,
                    PHONE_BATCH,
                )
                .await
            {
                log::warn!("older messages not requested: {error}");
                let _ = commands.send(Command::OlderFailed {
                    chat: chat.clone(),
                    error: format!("could not ask the phone for older messages: {error}"),
                });
            }
        });
    }

    // --- commands --------------------------------------------------------

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::SendText {
                chat,
                text,
                quoting,
            } => self.send_text(chat, text, quoting),
            Command::Composing { chat, composing } => {
                let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
                    return;
                };
                tokio::spawn(async move {
                    let result = if composing {
                        client.chatstate().send_composing(&jid).await
                    } else {
                        client.chatstate().send_paused(&jid).await
                    };
                    if let Err(error) = result {
                        log::debug!("chat state not sent: {error}");
                    }
                });
            }
            Command::MarkRead { chat, receipts } => self.mark_read(chat, receipts),
            Command::LoadChat { chat, before } => self.load_chat(chat, before),
            Command::FetchOlder(chat) => self.fetch_older(chat),
            Command::LoadUntil { chat, id, before } => self.load_until(chat, id, before),
            Command::Download { chat, message } => self.download(chat, message),
            Command::FetchAvatar { id, full } => self.fetch_avatar(id, full),
            Command::EditText { chat, id, text } => self.edit_text(chat, id, text),
            Command::Revoke { chat, id } => self.revoke(chat, id),
            Command::DeleteLocal { chat, id } => {
                if let Ok(true) = self.archive.delete_message(&chat, &id) {
                    self.emit(Event::MessageDeleted {
                        chat: chat.clone(),
                        id,
                    });
                    self.emit_chat(&chat);
                }
            }
            Command::PickFiles(chat) => {
                let commands = self.commands.clone();
                tokio::task::spawn_blocking(move || {
                    let paths = rfd::FileDialog::new()
                        .set_title("Send to WhatsApp")
                        .pick_files()
                        .unwrap_or_default();
                    let _ = commands.send(Command::Picked { chat, paths });
                });
            }
            Command::Picked { chat, paths } => self.emit(Event::Picked { chat, paths }),
            Command::SendFiles {
                chat,
                paths,
                caption,
            } => {
                self.send_files(chat, paths, caption);
            }
            Command::SendImage {
                chat,
                width,
                height,
                rgba,
                caption,
            } => self.send_pasted_image(chat, width, height, rgba, caption),
            Command::Outbound { chat, row, raw } => self.outbound(chat, *row, raw),
            Command::SendSticker { chat, path } => self.send_sticker(chat, path),
            Command::SendGif { chat, gif } => self.send_gif(chat, gif),
            Command::SearchGifs { query, key } => {
                let commands = self.commands.clone();
                let dir = self.dirs.cache.join("gifs");
                tokio::task::spawn_blocking(move || {
                    let results = search_gifs(&query, &key, &dir);
                    let _ = commands.send(Command::GifResults { query, results });
                });
            }
            Command::GifResults { query, results } => {
                self.emit(Event::Gifs { query, results });
            }
            Command::RecentStickers => {
                self.fetch_missing_stickers();
                self.emit_stickers();
            }
            Command::StickerFetched { hash, result } => {
                self.sticker_fetches.remove(&hash);
                match result {
                    Ok(path) => {
                        if let Err(error) = self.archive.set_sticker_path(&hash, &path) {
                            log::warn!("could not file sticker {hash}: {error}");
                        }
                    }
                    Err(error) => log::warn!("sticker {hash} could not be fetched: {error}"),
                }
                self.emit_stickers();
            }
            Command::MeInfo { about } => {
                self.me_about = about;
                match &self.me_about {
                    Some(about) => {
                        let _ = self.archive.set_meta("me_about", about);
                    }
                    None => {
                        let _ = self.archive.set_meta("me_about", "");
                    }
                }
                self.emit(Event::Me {
                    id: self.me(),
                    name: self.me_name.clone(),
                    about: self.me_about.clone(),
                });
            }
            Command::React {
                chat,
                message,
                emoji,
            } => self.react(chat, message, emoji),
            Command::SetArchived(chat, archived) => {
                let _ = self.archive.set_archived(&chat, archived);
                self.emit_chat(&chat);
                self.tell_phone(&chat, move |client, jid| async move {
                    if archived {
                        client.chat_actions().archive_chat(&jid, None).await
                    } else {
                        client.chat_actions().unarchive_chat(&jid, None).await
                    }
                    .map_err(|error| error.to_string())
                });
            }
            Command::SetPinned(chat, pinned) => {
                let _ = self.archive.set_pinned(&chat, pinned);
                self.emit_chat(&chat);
                self.tell_phone(&chat, move |client, jid| async move {
                    if pinned {
                        client.chat_actions().pin_chat(&jid).await
                    } else {
                        client.chat_actions().unpin_chat(&jid).await
                    }
                    .map_err(|error| error.to_string())
                });
            }
            Command::SetMuted(chat, until) => {
                let _ = self.archive.set_muted(&chat, until);
                self.emit_chat(&chat);
                self.tell_phone(&chat, move |client, jid| async move {
                    match until {
                        None => client.chat_actions().unmute_chat(&jid).await,
                        Some(0) => client.chat_actions().mute_chat(&jid).await,
                        Some(seconds) => {
                            client
                                .chat_actions()
                                .mute_chat_until(&jid, seconds * 1000)
                                .await
                        }
                    }
                    .map_err(|error| error.to_string())
                });
            }
            Command::PairWithPhone(phone) => {
                let Some(client) = self.client.clone() else {
                    self.emit(Event::Error("Not connected to WhatsApp yet".to_owned()));
                    return;
                };
                self.pairing_phone = Some(phone.clone());
                self.pair_code = None;
                let status = self.unlinked();
                self.set_status(status);
                let commands = self.commands.clone();
                tokio::spawn(async move {
                    let result = client
                        .pair_with_code(PairCodeOptions {
                            phone_number: phone,
                            ..Default::default()
                        })
                        .await
                        .map_err(|error| error.to_string());
                    let _ = commands.send(Command::PairCode { result });
                });
            }
            Command::PairCode { result } => match result {
                Ok(code) => {
                    self.pair_code = Some(code);
                    let status = self.unlinked();
                    self.set_status(status);
                }
                Err(error) => {
                    self.pairing_phone = None;
                    self.emit(Event::Error(format!("Pairing by phone failed: {error}")));
                    let status = self.unlinked();
                    self.set_status(status);
                }
            },
            Command::Unlink => {
                if let Some(client) = self.client.clone() {
                    client.logout().await;
                } else {
                    self.on_logged_out().await;
                }
            }
            Command::Reconnect => {
                if let Some(client) = self.client.clone() {
                    tokio::spawn(async move { client.reconnect_immediately().await });
                } else {
                    self.start_bot().await;
                }
            }
            Command::Shutdown => {}
            Command::OlderFailed { chat, error } => {
                self.pending_older.remove(&chat);
                self.emit(Event::OlderFetched { chat, more: true });
                self.emit(Event::Error(error));
            }
            Command::GroupInfoFailed { chat } => {
                self.group_info_requested.remove(&chat);
            }
            Command::Sent { chat, id, error } => {
                if id.is_empty() {
                    // Not a message: an errand in the chat failed.
                    if let Some(error) = error {
                        self.emit(Event::Error(error));
                    }
                    return;
                }
                let status = match &error {
                    Some(_) => Delivery::Failed,
                    None => Delivery::Sent,
                };
                let _ = self.archive.set_status(&chat, &id, status);
                self.emit_message(&chat, &id);
                self.emit_chat(&chat);
                if let Some(error) = error {
                    self.emit(Event::Error(format!("Message not sent: {error}")));
                }
            }
            Command::Downloaded { chat, id, result } => {
                if let Ok(path) = &result {
                    let _ = self.archive.set_media_path(&chat, &id, path);
                }
                let for_picker = self.sticker_downloads.remove(&(chat.clone(), id.clone()));
                self.emit(Event::Media {
                    chat,
                    message: id,
                    result,
                });
                if for_picker {
                    self.emit_stickers();
                }
            }
            Command::AvatarFetched { id, full, path } => {
                self.emit(Event::Avatar { id, full, path })
            }
            Command::AvatarFailed { id, full } => {
                *self.pending_avatars.entry((id, full)).or_insert(0) += 1;
            }
            Command::GroupInfo {
                chat,
                name,
                participants,
                read_only,
            } => {
                let _ =
                    self.archive
                        .set_group_info(&chat, name.as_deref(), &participants, read_only);
                self.emit_chat(&chat);
            }
        }
    }

    fn send_text(&mut self, chat: ChatId, text: String, quoting: Option<String>) {
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let mut quoted_row = None;
        let message = match quoting.as_deref().and_then(|id| {
            let raw = self.archive.raw(&chat, id).ok().flatten()?;
            let quoted = wa::Message::decode_from_slice(&raw).ok()?;
            let row = self.archive.message(&chat, id).ok().flatten()?;
            Some((quoted, row))
        }) {
            Some((quoted, row)) => {
                let sender = Self::jid_of(&row.sender).unwrap_or_else(|| jid.clone());
                let context = whatsapp_rust::wacore::proto_helpers::build_quote_context_with_info(
                    row.id.clone(),
                    &sender,
                    &jid,
                    &jid,
                    &quoted,
                );
                quoted_row = Some(row);
                wa::Message::text_with_context(text.clone(), context)
            }
            None => wa::Message::text(text.clone()),
        };
        let id = client.generate_message_id();
        let row = Message {
            id: id.clone(),
            chat: chat.clone(),
            sender: self.me(),
            sender_name: None,
            from_me: true,
            timestamp: crate::util::now(),
            content: Content::text(text),
            status: Delivery::Pending,
            quoted: quoted_row.map(|row| Quoted {
                mentions: row.mentions.clone(),
                id: row.id,
                sender_name: if row.from_me {
                    Some("You".to_owned())
                } else {
                    row.sender_name
                        .clone()
                        .or_else(|| self.name_for(&row.sender))
                },
                sender: row.sender,
                summary: row.content.summary(),
            }),
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        };
        self.store_message(row, Some(message.encode_to_vec()), None);
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let options = SendOptions::default().with_message_id(id.clone());
            let error = client
                .send_message_with_options(jid, message, options)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = commands.send(Command::Sent { chat, id, error });
        });
    }

    fn mark_read(&mut self, chat: ChatId, receipts: bool) {
        let Ok(Some(row)) = self.archive.chat(&chat) else {
            return;
        };
        let _ = self.archive.mark_read(&chat);
        self.emit_chat(&chat);
        if row.unread == 0 || !receipts {
            return;
        }
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            return;
        };
        let Ok(ids) = self.archive.unread_incoming(&chat, row.unread) else {
            return;
        };
        let is_group = jid.is_group();
        let mut by_sender: HashMap<Option<String>, Vec<String>> = HashMap::new();
        for (id, sender) in ids {
            by_sender
                .entry(is_group.then_some(sender))
                .or_default()
                .push(id);
        }
        tokio::spawn(async move {
            for (sender, ids) in by_sender {
                let sender = sender.and_then(|sender| sender.parse::<Jid>().ok());
                let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                if let Err(error) = client.mark_as_read(&jid, sender.as_ref(), &ids).await {
                    log::debug!("read receipt not sent: {error}");
                }
            }
        });
    }

    /// Brings a stored row up to date with what is known now: a quote
    /// filed under a privacy id before its number was learned, or before
    /// the name was.
    fn polish(&self, message: &mut Message) {
        if let Some(quoted) = message.quoted.as_mut() {
            let sender = self.canonical_str(&quoted.sender);
            if sender != quoted.sender || quoted.sender_name.is_none() {
                quoted.sender_name = self.name_for(&sender);
                quoted.sender = sender;
            }
            quoted.summary = self.pn_tokens(&quoted.summary);
            for mention in &mut quoted.mentions {
                mention.id = self.canonical_str(&mention.id);
            }
            if quoted.mentions.is_empty() {
                quoted.mentions = self.mention_tokens(&quoted.summary);
            }
        }
        for mention in &mut message.mentions {
            mention.id = self.canonical_str(&mention.id);
        }
    }

    fn load_chat(&mut self, chat: ChatId, before: Option<super::PageKey>) {
        match self.archive.messages(
            &chat,
            before.as_ref().map(|(time, id)| (*time, id.as_str())),
            PAGE + 1,
        ) {
            Ok(mut messages) => {
                let complete = messages.len() <= PAGE;
                if !complete {
                    messages.remove(0);
                }
                for message in &mut messages {
                    self.polish(message);
                }
                self.emit(Event::Messages {
                    chat: chat.clone(),
                    messages,
                    older: before.is_some(),
                    complete,
                });
            }
            Err(error) => self.emit(Event::Error(format!("Could not read the chat: {error}"))),
        }
        if before.is_none() && ChatKind::from_id(&chat) == ChatKind::Group {
            // Opening a group is the moment its members matter.
            self.request_group_info(&chat, false);
        }
        if before.is_none()
            && ChatKind::from_id(&chat) == ChatKind::Direct
            && chat != self.me()
            && self.presence_subscribed.insert(chat.clone())
            && let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat))
        {
            tokio::spawn(async move {
                if let Err(error) = client.presence().subscribe(jid).await {
                    log::debug!("presence not subscribed: {error}");
                }
            });
        }
    }

    fn download(&mut self, chat: ChatId, id: String) {
        let Some(client) = self.client.clone() else {
            self.emit(Event::Media {
                chat,
                message: id,
                result: Err("Not connected to WhatsApp".to_owned()),
            });
            return;
        };
        let raw = self.archive.raw(&chat, &id).ok().flatten();
        let Some(message) = raw.and_then(|raw| wa::Message::decode_from_slice(&raw).ok()) else {
            self.emit(Event::Media {
                chat,
                message: id,
                result: Err("The attachment's keys are not in the archive".to_owned()),
            });
            return;
        };
        let base = message.get_base_message().clone();
        let (downloadable, mime, file_name): (Box<dyn Downloadable>, String, Option<String>) =
            if let Some(image) = base.image_message.as_option() {
                (
                    Box::new(image.clone()),
                    image.mimetype.clone().unwrap_or_default(),
                    None,
                )
            } else if let Some(video) = base
                .video_message
                .as_option()
                .or(base.ptv_message.as_option())
            {
                (
                    Box::new(video.clone()),
                    video.mimetype.clone().unwrap_or_default(),
                    None,
                )
            } else if let Some(audio) = base.audio_message.as_option() {
                (
                    Box::new(audio.clone()),
                    audio.mimetype.clone().unwrap_or_default(),
                    None,
                )
            } else if let Some(document) = base.document_message.as_option() {
                (
                    Box::new(document.clone()),
                    document.mimetype.clone().unwrap_or_default(),
                    document.file_name.clone(),
                )
            } else if let Some(sticker) = base.sticker_message.as_option() {
                (
                    Box::new(sticker.clone()),
                    sticker.mimetype.clone().unwrap_or_default(),
                    None,
                )
            } else {
                self.emit(Event::Media {
                    chat,
                    message: id,
                    result: Err("Nothing to download in this message".to_owned()),
                });
                return;
            };
        let dir = self.dirs.media_cache_dir();
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let result = async {
                let bytes = client
                    .download(&*downloadable)
                    .await
                    .map_err(|error| error.to_string())?;
                let path = media_path(&dir, &chat, &id, &mime, file_name.as_deref());
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|error| error.to_string())?;
                tokio::fs::write(&path, &bytes)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(path)
            }
            .await;
            let _ = commands.send(Command::Downloaded { chat, id, result });
        });
    }

    /// Fetches what the picker lacks a file for: the phone's recent
    /// stickers, and sticker messages nobody has opened yet.
    fn fetch_missing_stickers(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let phone = match self.archive.phone_stickers() {
            Ok(list) => list,
            Err(error) => {
                log::warn!("could not list the phone's stickers: {error}");
                Vec::new()
            }
        };
        let dir = self.dirs.sticker_cache_dir();
        for sticker in phone.into_iter().filter(|sticker| sticker.path.is_none()) {
            if !self.sticker_fetches.insert(sticker.hash.clone()) {
                continue;
            }
            let Ok(meta) = wa::StickerMetadata::decode_from_slice(&sticker.raw) else {
                self.sticker_fetches.remove(&sticker.hash);
                continue;
            };
            let client = client.clone();
            let commands = self.commands.clone();
            let dir = dir.clone();
            let hash = sticker.hash;
            tokio::spawn(async move {
                let result = async {
                    let bytes = client
                        .download(&PhoneSticker(meta))
                        .await
                        .map_err(|error| error.to_string())?;
                    tokio::fs::create_dir_all(&dir)
                        .await
                        .map_err(|error| error.to_string())?;
                    let path = dir.join(format!("{hash}.webp"));
                    tokio::fs::write(&path, &bytes)
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(path)
                }
                .await;
                let _ = commands.send(Command::StickerFetched { hash, result });
            });
        }
        match self.archive.stickers_without_file(STICKER_FETCH_LIMIT) {
            Ok(list) => {
                for (chat, id) in list {
                    if self.sticker_downloads.insert((chat.clone(), id.clone())) {
                        self.download(chat, id);
                    }
                }
            }
            Err(error) => log::warn!("could not list unfetched stickers: {error}"),
        }
    }

    /// The picker's list: every sticker with a file, the phone's recent
    /// ones and those in the archive, most recently used first, one entry
    /// per distinct sticker.
    fn emit_stickers(&mut self) {
        let mut seen = HashSet::new();
        let mut list: Vec<(i64, PathBuf)> = Vec::new();
        if let Ok(phone) = self.archive.phone_stickers() {
            for sticker in phone {
                if let Some(path) = sticker.path
                    && path.exists()
                    && seen.insert(sticker.hash)
                {
                    list.push((sticker.last_used, path));
                }
            }
        }
        match self.archive.recent_stickers(80) {
            Ok(rows) => {
                for sticker in rows {
                    let hash = sticker
                        .raw
                        .as_deref()
                        .and_then(|raw| wa::Message::decode_from_slice(raw).ok())
                        .and_then(|message| {
                            let base = message.get_base_message();
                            let sticker = base.sticker_message.as_option()?;
                            sticker_hash(
                                sticker.file_sha256.as_deref(),
                                sticker.file_enc_sha256.as_deref(),
                            )
                        })
                        .unwrap_or_else(|| sticker.path.display().to_string());
                    if seen.insert(hash) {
                        list.push((sticker.last_used, sticker.path));
                    }
                }
            }
            Err(error) => log::warn!("could not list stickers: {error}"),
        }
        list.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
        self.emit(Event::Stickers(
            list.into_iter().map(|(_, path)| path).collect(),
        ));
    }

    fn avatar_file(&self, id: &str, full: bool) -> PathBuf {
        self.dirs.avatar_file(id, full)
    }

    fn fetch_avatar(&mut self, id: String, full: bool) {
        let path = self.avatar_file(&id, full);
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age < AVATAR_FRESH)
        {
            let path = (metadata.len() > 0).then_some(path);
            self.emit(Event::Avatar { id, full, path });
            return;
        }
        // Our own picture answers to whichever of our ids WhatsApp prefers.
        let candidates: Vec<Jid> = if self.is_me(&id) || id == self.me() {
            [self.me_pn.clone(), self.me_lid.clone()]
                .into_iter()
                .flatten()
                .filter_map(|id| Self::jid_of(&id))
                .collect()
        } else {
            Self::jid_of(&id).into_iter().collect()
        };
        if candidates.is_empty() {
            self.emit(Event::Avatar {
                id,
                full,
                path: None,
            });
            return;
        }
        let connected = self
            .client
            .as_ref()
            .is_some_and(|client| client.is_connected());
        let Some(client) = self.client.clone().filter(|_| connected) else {
            // Not connected yet: the interface keeps waiting, and the lookup
            // runs once the link is up.
            self.pending_avatars.entry((id, full)).or_insert(0);
            return;
        };
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let fetched = async {
                let mut picture = None;
                let mut failed = false;
                'lookup: for jid in &candidates {
                    for preview in [!full, false] {
                        match client.contacts().get_profile_picture(jid, preview).await {
                            Ok(Some(found)) => {
                                picture = Some(found);
                                break 'lookup;
                            }
                            Ok(None) => {}
                            Err(error) => {
                                log::debug!("picture lookup failed: {error}");
                                failed = true;
                            }
                        }
                    }
                }
                let Some(picture) = picture else {
                    return if failed {
                        Err("lookup failed".to_owned())
                    } else {
                        Ok(None)
                    };
                };
                let url = picture.url;
                let bytes = tokio::task::spawn_blocking(move || {
                    ureq::get(&url)
                        .call()
                        .and_then(|mut response| response.body_mut().read_to_vec())
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())??;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                tokio::fs::write(&path, &bytes)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok::<Option<PathBuf>, String>(Some(path.clone()))
            }
            .await;
            match fetched {
                Ok(path) => {
                    let _ = commands.send(Command::AvatarFetched { id, full, path });
                }
                Err(error) => {
                    log::debug!("no picture for {id} yet: {error}");
                    let _ = commands.send(Command::AvatarFailed { id, full });
                }
            }
        });
    }

    /// Runs the picture lookups that waited for the link, or failed.
    fn retry_avatars(&mut self) {
        if !self
            .client
            .as_ref()
            .is_some_and(|client| client.is_connected())
        {
            return;
        }
        let due: Vec<(String, bool)> = self.pending_avatars.keys().cloned().collect();
        for (id, full) in due {
            let attempts = self
                .pending_avatars
                .remove(&(id.clone(), full))
                .unwrap_or(0);
            if attempts >= 3 {
                self.emit(Event::Avatar {
                    id,
                    full,
                    path: None,
                });
                continue;
            }
            self.fetch_avatar(id, full);
        }
    }

    /// Everything between a quoted message and what is loaded, so the view
    /// can scroll to it.
    fn load_until(&mut self, chat: ChatId, id: String, before: super::PageKey) {
        let Ok(Some(target)) = self.archive.message(&chat, &id) else {
            self.emit(Event::Messages {
                chat: chat.clone(),
                messages: Vec::new(),
                older: true,
                complete: false,
            });
            self.emit(Event::Error(
                "That message is not on this computer".to_owned(),
            ));
            return;
        };
        match self
            .archive
            .messages_range(&chat, target.timestamp, (before.0, &before.1), 2000)
        {
            Ok(mut messages) => {
                for message in &mut messages {
                    self.polish(message);
                }
                self.emit(Event::Messages {
                    chat,
                    messages,
                    older: true,
                    complete: false,
                });
            }
            Err(error) => self.emit(Event::Error(format!("Could not read the chat: {error}"))),
        }
    }

    fn edit_text(&mut self, chat: ChatId, id: String, text: String) {
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let content = Content::text(text.clone());
        if let Ok(true) = self.archive.set_content(&chat, &id, &content, true) {
            self.emit_message(&chat, &id);
            self.emit_chat(&chat);
        }
        let commands = self.commands.clone();
        tokio::spawn(async move {
            if let Err(error) = client
                .edit_message(jid, id.clone(), wa::Message::text(text))
                .await
            {
                let _ = commands.send(Command::Sent {
                    chat,
                    id: String::new(),
                    error: Some(format!("the edit was not sent: {error}")),
                });
            }
        });
    }

    fn revoke(&mut self, chat: ChatId, id: String) {
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        if let Ok(true) = self
            .archive
            .set_content(&chat, &id, &Content::Revoked, false)
        {
            self.emit_message(&chat, &id);
            self.emit_chat(&chat);
        }
        let commands = self.commands.clone();
        tokio::spawn(async move {
            if let Err(error) = client.revoke_message(jid, id, RevokeType::Sender).await {
                let _ = commands.send(Command::Sent {
                    chat,
                    id: String::new(),
                    error: Some(format!("the message was not deleted for everyone: {error}")),
                });
            }
        });
    }

    fn send_files(&mut self, chat: ChatId, paths: Vec<PathBuf>, caption: Option<String>) {
        for (index, path) in paths.into_iter().enumerate() {
            let Some(client) = self.client.clone() else {
                self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
                return;
            };
            let commands = self.commands.clone();
            let chat = chat.clone();
            let dir = self.dirs.media_cache_dir();
            let me = self.me();
            // The caption goes with the first file, as on the phone.
            let caption = if index == 0 { caption.clone() } else { None };
            tokio::spawn(async move {
                let outcome = async {
                    let bytes = tokio::fs::read(&path)
                        .await
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    let mime = mime_guess2::from_path(&path)
                        .first_or_octet_stream()
                        .to_string();
                    let file_name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned());
                    let prepared =
                        prepare_media(&client, bytes, &mime, file_name.as_deref(), false).await?;
                    file_outbound(&client, &chat, &me, &dir, prepared, caption).await
                }
                .await;
                match outcome {
                    Ok((row, raw)) => {
                        let _ = commands.send(Command::Outbound {
                            chat,
                            row: Box::new(row),
                            raw,
                        });
                    }
                    Err(error) => {
                        let _ = commands.send(Command::Sent {
                            chat,
                            id: String::new(),
                            error: Some(format!("could not send the file: {error}")),
                        });
                    }
                }
            });
        }
    }

    fn send_pasted_image(
        &mut self,
        chat: ChatId,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        caption: Option<String>,
    ) {
        let Some(client) = self.client.clone() else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let commands = self.commands.clone();
        let dir = self.dirs.media_cache_dir();
        let me = self.me();
        tokio::spawn(async move {
            let outcome = async {
                let encoded = tokio::task::spawn_blocking(move || {
                    let image = image::RgbaImage::from_raw(width, height, rgba)
                        .ok_or_else(|| "the clipboard picture is malformed".to_owned())?;
                    encode_jpeg(&image::DynamicImage::ImageRgba8(image), 88)
                })
                .await
                .map_err(|error| error.to_string())??;
                let prepared = prepare_media(&client, encoded, "image/jpeg", None, false).await?;
                file_outbound(&client, &chat, &me, &dir, prepared, caption).await
            }
            .await;
            match outcome {
                Ok((row, raw)) => {
                    let _ = commands.send(Command::Outbound {
                        chat,
                        row: Box::new(row),
                        raw,
                    });
                }
                Err(error) => {
                    let _ = commands.send(Command::Sent {
                        chat,
                        id: String::new(),
                        error: Some(format!("could not send the picture: {error}")),
                    });
                }
            }
        });
    }

    fn send_sticker(&mut self, chat: ChatId, path: PathBuf) {
        let Some(client) = self.client.clone() else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let commands = self.commands.clone();
        let dir = self.dirs.media_cache_dir();
        let me = self.me();
        tokio::spawn(async move {
            let outcome = async {
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|error| error.to_string())?;
                let prepared = prepare_sticker(&client, bytes).await?;
                file_outbound(&client, &chat, &me, &dir, prepared, None).await
            }
            .await;
            match outcome {
                Ok((row, raw)) => {
                    let _ = commands.send(Command::Outbound {
                        chat,
                        row: Box::new(row),
                        raw,
                    });
                }
                Err(error) => {
                    let _ = commands.send(Command::Sent {
                        chat,
                        id: String::new(),
                        error: Some(format!("could not send the sticker: {error}")),
                    });
                }
            }
        });
    }

    fn send_gif(&mut self, chat: ChatId, gif: Gif) {
        let Some(client) = self.client.clone() else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let commands = self.commands.clone();
        let dir = self.dirs.media_cache_dir();
        let me = self.me();
        tokio::spawn(async move {
            let outcome = async {
                let url = gif.mp4.clone();
                let bytes = tokio::task::spawn_blocking(move || {
                    ureq::get(&url)
                        .call()
                        .and_then(|mut response| response.body_mut().read_to_vec())
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())??;
                let mut prepared = prepare_media(&client, bytes, "video/mp4", None, true).await?;
                if let Content::Video { media, .. } = &mut prepared.content {
                    media.width = Some(gif.width);
                    media.height = Some(gif.height);
                }
                if let Some(video) = prepared.message.video_message.as_option_mut() {
                    video.width = Some(gif.width);
                    video.height = Some(gif.height);
                }
                file_outbound(&client, &chat, &me, &dir, prepared, None).await
            }
            .await;
            match outcome {
                Ok((row, raw)) => {
                    let _ = commands.send(Command::Outbound {
                        chat,
                        row: Box::new(row),
                        raw,
                    });
                }
                Err(error) => {
                    let _ = commands.send(Command::Sent {
                        chat,
                        id: String::new(),
                        error: Some(format!("could not send the GIF: {error}")),
                    });
                }
            }
        });
    }

    /// Files an uploaded attachment's message and sends it.
    fn outbound(&mut self, chat: ChatId, row: Message, raw: Vec<u8>) {
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let Ok(message) = wa::Message::decode_from_slice(&raw) else {
            self.emit(Event::Error(
                "The attachment could not be encoded".to_owned(),
            ));
            return;
        };
        let id = row.id.clone();
        self.store_message(row, Some(raw), None);
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let options = SendOptions::default().with_message_id(id.clone());
            let error = client
                .send_message_with_options(jid, message, options)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = commands.send(Command::Sent { chat, id, error });
        });
    }

    fn react(&mut self, chat: ChatId, id: String, emoji: String) {
        let (Some(client), Some(jid)) = (self.client.clone(), Self::jid_of(&chat)) else {
            self.emit(Event::Error("Not connected to WhatsApp".to_owned()));
            return;
        };
        let Ok(Some(target)) = self.archive.message(&chat, &id) else {
            return;
        };
        let me = self.me();
        if let Ok(Some(updated)) = self.archive.set_reaction(&chat, &id, &me, true, &emoji) {
            self.emit(Event::MessageUpdated(Box::new(updated)));
        }
        let key = wa::MessageKey {
            remote_jid: Some(chat.clone()),
            from_me: Some(target.from_me),
            id: Some(id),
            participant: (jid.is_group() && !target.from_me).then(|| target.sender.clone()),
        };
        tokio::spawn(async move {
            if let Err(error) = client.send_reaction(jid, key, &emoji).await {
                log::warn!("reaction not sent: {error}");
            }
        });
    }
}

// --- free helpers ----------------------------------------------------------

/// What a chat is called before anyone tells us: the phone number, or the
/// bare id.
fn fallback_name(id: &str) -> String {
    match crate::model::phone_of(id) {
        Some(digits) => crate::util::phone(digits),
        None if ChatKind::from_id(id) == ChatKind::Group => "Group".to_owned(),
        None => id.split('@').next().unwrap_or(id).to_owned(),
    }
}

/// WhatsApp writes some timestamps in milliseconds; normalise to seconds.
fn seconds(timestamp: i64) -> i64 {
    if timestamp > 100_000_000_000 {
        timestamp / 1000
    } else {
        timestamp.max(0)
    }
}

fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn extension_for(mime: &str, file_name: Option<&str>) -> String {
    if let Some(extension) = file_name
        .and_then(|name| Path::new(name).extension())
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty() && extension.len() <= 8)
    {
        return extension.to_ascii_lowercase();
    }
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "video/3gpp" => "3gp",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "audio/wav" => "wav",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => mime.rsplit('/').next().unwrap_or("bin"),
    }
    .to_owned()
}

fn media_path(dir: &Path, chat: &str, id: &str, mime: &str, file_name: Option<&str>) -> PathBuf {
    let extension = extension_for(mime, file_name);
    let stem = match file_name.and_then(|name| Path::new(name).file_stem()?.to_str()) {
        Some(name) => format!("{}-{}", sanitize(id), sanitize(name)),
        None => format!("{}-{}", sanitize(chat), sanitize(id)),
    };
    dir.join(format!("{stem}.{extension}"))
}

fn media(
    mime: Option<&String>,
    size: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
) -> Media {
    Media {
        mime: mime.cloned().unwrap_or_default(),
        size: size.unwrap_or(0),
        width,
        height,
        path: None,
        state: Default::default(),
    }
}

fn non_empty(text: &Option<String>) -> Option<String> {
    text.clone().filter(|text| !text.trim().is_empty())
}

/// The context (quote, mentions) a message carries, wherever it sits.
fn context_of(base: &wa::Message) -> Option<&wa::ContextInfo> {
    if let Some(text) = base.extended_text_message.as_option() {
        return text.context_info.as_option();
    }
    if let Some(image) = base.image_message.as_option() {
        return image.context_info.as_option();
    }
    if let Some(video) = base.video_message.as_option() {
        return video.context_info.as_option();
    }
    if let Some(audio) = base.audio_message.as_option() {
        return audio.context_info.as_option();
    }
    if let Some(document) = base.document_message.as_option() {
        return document.context_info.as_option();
    }
    if let Some(sticker) = base.sticker_message.as_option() {
        return sticker.context_info.as_option();
    }
    if let Some(location) = base.location_message.as_option() {
        return location.context_info.as_option();
    }
    if let Some(contact) = base.contact_message.as_option() {
        return contact.context_info.as_option();
    }
    None
}

/// The JIDs a message names with `@`, as WhatsApp wrote them.
fn mentioned_of(base: &wa::Message) -> Vec<String> {
    context_of(base)
        .map(|context| context.mentioned_jid.clone())
        .unwrap_or_default()
}

fn forwarded_of(base: &wa::Message) -> bool {
    context_of(base).is_some_and(|context| {
        context.is_forwarded.unwrap_or(false) || context.forwarding_score.unwrap_or(0) > 0
    })
}

/// The first web address in a text, for a preview whose message did not
/// say which link it was about.
fn first_link(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|token| token.starts_with("http://") || token.starts_with("https://"))
        .map(|token| token.trim_end_matches(['.', ',', ')', ']']).to_owned())
}

/// The small picture that travels with an attachment or a link preview.
fn thumbnail_of(base: &wa::Message) -> Option<Vec<u8>> {
    let bytes = if let Some(image) = base.image_message.as_option() {
        image.jpeg_thumbnail.clone()
    } else if let Some(video) = base
        .video_message
        .as_option()
        .or(base.ptv_message.as_option())
    {
        video.jpeg_thumbnail.clone()
    } else if let Some(document) = base.document_message.as_option() {
        document.jpeg_thumbnail.clone()
    } else if let Some(text) = base.extended_text_message.as_option() {
        text.jpeg_thumbnail.clone()
    } else {
        None
    };
    bytes.filter(|bytes| !bytes.is_empty())
}

/// What a message shows, or `None` for protocol traffic the user never
/// sees.
fn classify(base: &wa::Message) -> Option<Content> {
    if let Some(text) = base.text_content() {
        let preview = base.extended_text_message.as_option().and_then(|extended| {
            let title = non_empty(&extended.title);
            let description = non_empty(&extended.description);
            let has_picture = extended
                .jpeg_thumbnail
                .as_ref()
                .is_some_and(|bytes| !bytes.is_empty());
            if title.is_none() && description.is_none() && !has_picture {
                return None;
            }
            let url = non_empty(&extended.matched_text).or_else(|| first_link(text))?;
            let url = if url.contains("://") {
                url
            } else {
                format!("https://{url}")
            };
            Some(LinkPreview {
                url,
                title,
                description,
            })
        });
        return Some(Content::Text {
            text: text.to_owned(),
            preview,
        });
    }
    if let Some(image) = base.image_message.as_option() {
        return Some(Content::Image {
            caption: non_empty(&image.caption),
            media: media(
                image.mimetype.as_ref(),
                image.file_length,
                image.width,
                image.height,
            ),
        });
    }
    if let Some(video) = base
        .video_message
        .as_option()
        .or(base.ptv_message.as_option())
    {
        return Some(Content::Video {
            caption: non_empty(&video.caption),
            media: media(
                video.mimetype.as_ref(),
                video.file_length,
                video.width,
                video.height,
            ),
            seconds: video.seconds,
            gif: video.gif_playback.unwrap_or(false),
        });
    }
    if let Some(audio) = base.audio_message.as_option() {
        return Some(Content::Audio {
            media: media(audio.mimetype.as_ref(), audio.file_length, None, None),
            seconds: audio.seconds,
            voice_note: audio.ptt.unwrap_or(false),
        });
    }
    if let Some(document) = base.document_message.as_option() {
        let file_name = non_empty(&document.file_name)
            .or_else(|| non_empty(&document.title))
            .unwrap_or_else(|| "Document".to_owned());
        return Some(Content::Document {
            media: media(document.mimetype.as_ref(), document.file_length, None, None),
            file_name,
            caption: non_empty(&document.caption),
            pages: document.page_count,
        });
    }
    if let Some(sticker) = base.sticker_message.as_option() {
        return Some(Content::Sticker {
            media: media(
                sticker.mimetype.as_ref(),
                sticker.file_length,
                sticker.width,
                sticker.height,
            ),
            animated: sticker.is_animated.unwrap_or(false),
        });
    }
    if let Some(location) = base.location_message.as_option() {
        return Some(Content::Location {
            latitude: location.degrees_latitude.unwrap_or(0.0),
            longitude: location.degrees_longitude.unwrap_or(0.0),
            name: non_empty(&location.name),
            address: non_empty(&location.address),
        });
    }
    if let Some(live) = base.live_location_message.as_option() {
        return Some(Content::Location {
            latitude: live.degrees_latitude.unwrap_or(0.0),
            longitude: live.degrees_longitude.unwrap_or(0.0),
            name: Some("Live location".to_owned()),
            address: None,
        });
    }
    if let Some(contact) = base.contact_message.as_option() {
        return Some(Content::Contact {
            display_name: non_empty(&contact.display_name).unwrap_or_else(|| "Contact".to_owned()),
            vcard: contact.vcard.clone().unwrap_or_default(),
        });
    }
    if let Some(contacts) = base.contacts_array_message.as_option() {
        let count = contacts.contacts.len();
        return Some(Content::Contact {
            display_name: non_empty(&contacts.display_name)
                .unwrap_or_else(|| format!("{count} contacts")),
            vcard: contacts
                .contacts
                .iter()
                .filter_map(|contact| contact.vcard.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    if let Some(poll) = base
        .poll_creation_message
        .as_option()
        .or(base.poll_creation_message_v2.as_option())
        .or(base.poll_creation_message_v3.as_option())
    {
        return Some(Content::Poll {
            question: non_empty(&poll.name).unwrap_or_else(|| "Poll".to_owned()),
            options: poll
                .options
                .iter()
                .filter_map(|option| non_empty(&option.option_name))
                .collect(),
        });
    }
    let unsupported = |what: &str| {
        Some(Content::Unsupported {
            what: what.to_owned(),
        })
    };
    if base.album_message.is_set() {
        return None;
    }
    if base.group_invite_message.is_set() {
        return unsupported("group invite");
    }
    if base.event_message.is_set() {
        return unsupported("event");
    }
    if base.sticker_pack_message.is_set() {
        return unsupported("sticker pack");
    }
    if base.interactive_message.is_set()
        || base.buttons_message.is_set()
        || base.list_message.is_set()
        || base.template_message.is_set()
        || base.buttons_response_message.is_set()
        || base.list_response_message.is_set()
        || base.interactive_response_message.is_set()
        || base.template_button_reply_message.is_set()
    {
        return unsupported("interactive message");
    }
    if base.product_message.is_set() || base.order_message.is_set() {
        return unsupported("product");
    }
    if base.send_payment_message.is_set()
        || base.request_payment_message.is_set()
        || base.payment_invite_message.is_set()
        || base.invoice_message.is_set()
    {
        return unsupported("payment");
    }
    if base.call_log_messsage.is_set() || base.scheduled_call_creation_message.is_set() {
        return unsupported("call");
    }
    if base.lottie_sticker_message.is_set() {
        return unsupported("animated sticker");
    }
    if base.poll_update_message.is_set()
        || base.enc_reaction_message.is_set()
        || base.enc_comment_message.is_set()
        || base.enc_event_response_message.is_set()
        || base.keep_in_chat_message.is_set()
        || base.pin_in_chat_message.is_set()
        || base.sender_key_distribution_message.is_set()
        || base
            .fast_ratchet_key_sender_key_distribution_message
            .is_set()
        || base.sticker_sync_rmr_message.is_set()
        || base.message_context_info.is_set()
        || base.device_sent_message.is_set()
        || base.placeholder_message.is_set()
        || base.secret_encrypted_message.is_set()
        || base.message_history_bundle.is_set()
        || base.message_history_notice.is_set()
        || base.bot_invoke_message.is_set()
    {
        return None;
    }
    if *base == wa::Message::default() {
        return None;
    }
    unsupported("message")
}

/// An attachment ready to go: the protobuf WhatsApp gets, and what the
/// archive keeps.
struct Prepared {
    message: wa::Message,
    content: Content,
    thumbnail: Option<Vec<u8>>,
    bytes: Vec<u8>,
    mime: String,
    file_name: Option<String>,
}

fn encode_jpeg(image: &image::DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality);
    encoder
        .encode_image(&image.to_rgb8())
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

/// The little picture WhatsApp shows before an attachment is fetched.
fn thumbnail_jpeg(image: &image::DynamicImage) -> Option<Vec<u8>> {
    let small = image.thumbnail(THUMBNAIL_SIDE, THUMBNAIL_SIDE);
    encode_jpeg(&small, 60).ok()
}

/// Uploads a file and builds the message that carries it. Pictures go as
/// JPEG, which is what every WhatsApp client expects.
async fn prepare_media(
    client: &Client,
    bytes: Vec<u8>,
    mime: &str,
    file_name: Option<&str>,
    gif: bool,
) -> Result<Prepared, String> {
    let kind = mime.split('/').next().unwrap_or_default();
    let is_picture = matches!(
        mime,
        "image/jpeg" | "image/png" | "image/webp" | "image/bmp" | "image/tiff"
    );
    if is_picture {
        let decoded = tokio::task::spawn_blocking({
            let bytes = bytes.clone();
            move || image::load_from_memory(&bytes).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        let (width, height) = (decoded.width(), decoded.height());
        let jpeg = if mime == "image/jpeg" {
            bytes
        } else {
            encode_jpeg(&decoded, 88)?
        };
        let thumbnail = thumbnail_jpeg(&decoded);
        let upload = client
            .upload(jpeg.clone(), MediaType::Image, UploadOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        let mut message = image_message(
            upload,
            ImageOptions {
                caption: None,
                mimetype: Some("image/jpeg".to_owned()),
                jpeg_thumbnail: thumbnail.clone(),
                context_info: None,
            },
        );
        if let Some(image) = message.image_message.as_option_mut() {
            image.width = Some(width);
            image.height = Some(height);
        }
        return Ok(Prepared {
            message,
            content: Content::Image {
                caption: None,
                media: media(
                    Some(&"image/jpeg".to_owned()),
                    Some(jpeg.len() as u64),
                    Some(width),
                    Some(height),
                ),
            },
            thumbnail,
            bytes: jpeg,
            mime: "image/jpeg".to_owned(),
            file_name: None,
        });
    }
    let size = bytes.len() as u64;
    let mime_owned = mime.to_owned();
    if kind == "video" {
        let upload = client
            .upload(bytes.clone(), MediaType::Video, UploadOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        let message = video_message(
            upload,
            VideoOptions {
                mimetype: Some(mime_owned.clone()),
                gif_playback: Some(gif),
                ..Default::default()
            },
        );
        return Ok(Prepared {
            message,
            content: Content::Video {
                caption: None,
                media: media(Some(&mime_owned), Some(size), None, None),
                seconds: None,
                gif,
            },
            thumbnail: None,
            bytes,
            mime: mime_owned,
            file_name: file_name.map(str::to_owned),
        });
    }
    if kind == "audio" {
        let upload = client
            .upload(bytes.clone(), MediaType::Audio, UploadOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        let message = audio_message(
            upload,
            AudioOptions {
                mimetype: Some(mime_owned.clone()),
                ptt: Some(false),
                ..Default::default()
            },
        );
        return Ok(Prepared {
            message,
            content: Content::Audio {
                media: media(Some(&mime_owned), Some(size), None, None),
                seconds: None,
                voice_note: false,
            },
            thumbnail: None,
            bytes,
            mime: mime_owned,
            file_name: file_name.map(str::to_owned),
        });
    }
    let upload = client
        .upload(bytes.clone(), MediaType::Document, UploadOptions::default())
        .await
        .map_err(|error| error.to_string())?;
    let name = file_name.unwrap_or("file").to_owned();
    let message = document_message(
        upload,
        DocumentOptions {
            mimetype: Some(mime_owned.clone()),
            file_name: Some(name.clone()),
            title: Some(name.clone()),
            ..Default::default()
        },
    );
    Ok(Prepared {
        message,
        content: Content::Document {
            media: media(Some(&mime_owned), Some(size), None, None),
            file_name: name.clone(),
            caption: None,
            pages: None,
        },
        thumbnail: None,
        bytes,
        mime: mime_owned,
        file_name: Some(name),
    })
}

/// Uploads a WebP sticker and builds its message; the library has no
/// builder for stickers, so the fields are set by hand.
async fn prepare_sticker(client: &Client, bytes: Vec<u8>) -> Result<Prepared, String> {
    let (animated, width, height) = tokio::task::spawn_blocking({
        let bytes = bytes.clone();
        move || {
            let decoder = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(&bytes))
                .map_err(|error| error.to_string())?;
            let animated = decoder.has_animation();
            let (width, height) = image::ImageDecoder::dimensions(&decoder);
            Ok::<_, String>((animated, width, height))
        }
    })
    .await
    .map_err(|error| error.to_string())??;
    let upload = client
        .upload(bytes.clone(), MediaType::Sticker, UploadOptions::default())
        .await
        .map_err(|error| error.to_string())?;
    let message = wa::Message {
        sticker_message: MessageField::some(wa::message::StickerMessage {
            url: Some(upload.url),
            direct_path: Some(upload.direct_path),
            media_key: Some(upload.media_key.to_vec()),
            file_enc_sha256: Some(upload.file_enc_sha256.to_vec()),
            file_sha256: Some(upload.file_sha256.to_vec()),
            file_length: Some(upload.file_length),
            mimetype: Some("image/webp".to_owned()),
            media_key_timestamp: Some(upload.media_key_timestamp),
            is_animated: Some(animated),
            width: Some(width),
            height: Some(height),
            ..Default::default()
        }),
        ..Default::default()
    };
    Ok(Prepared {
        message,
        content: Content::Sticker {
            media: media(
                Some(&"image/webp".to_owned()),
                Some(bytes.len() as u64),
                Some(width),
                Some(height),
            ),
            animated,
        },
        thumbnail: None,
        bytes,
        mime: "image/webp".to_owned(),
        file_name: None,
    })
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Asks GIPHY, and fetches a still of each answer so the picker can show
/// it. An empty query lists what is trending.
fn search_gifs(query: &str, key: &str, dir: &Path) -> Result<Vec<Gif>, String> {
    if key.is_empty() {
        return Err("Add a GIPHY API key in Settings to search GIFs".to_owned());
    }
    let url = if query.trim().is_empty() {
        format!("https://api.giphy.com/v1/gifs/trending?api_key={key}&limit=24&rating=pg-13")
    } else {
        format!(
            "https://api.giphy.com/v1/gifs/search?api_key={key}&q={}&limit=24&rating=pg-13",
            percent_encode(query.trim())
        )
    };
    let body = ureq::get(&url)
        .call()
        .and_then(|mut response| response.body_mut().read_to_string())
        .map_err(|error| format!("GIPHY did not answer: {error}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| format!("GIPHY answered oddly: {error}"))?;
    if let Some(message) = json["meta"]["msg"].as_str()
        && json["meta"]["status"]
            .as_u64()
            .is_some_and(|status| status >= 400)
    {
        return Err(format!("GIPHY: {message}"));
    }
    let data = json["data"]
        .as_array()
        .ok_or_else(|| "GIPHY answered without results".to_owned())?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let mut gifs: Vec<(Gif, Option<String>)> = data
        .iter()
        .filter_map(|item| {
            let id = item["id"].as_str()?.to_owned();
            let images = &item["images"];
            let pick = |names: &[&str], field: &str| {
                names
                    .iter()
                    .find_map(|name| images[*name][field].as_str().map(str::to_owned))
            };
            let mp4 = pick(&["fixed_width", "downsized_small", "original"], "mp4")?;
            let still = pick(
                &[
                    "fixed_width_small_still",
                    "fixed_width_still",
                    "original_still",
                ],
                "url",
            );
            let number = |name: &str| {
                images["fixed_width"][name]
                    .as_str()
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(200)
            };
            Some((
                Gif {
                    id,
                    still: None,
                    mp4,
                    width: number("width"),
                    height: number("height"),
                },
                still,
            ))
        })
        .collect();
    std::thread::scope(|scope| {
        for (gif, still) in &mut gifs {
            let Some(url) = still.clone() else {
                continue;
            };
            let path = dir.join(format!("{}.jpg", sanitize(&gif.id)));
            if path.exists() {
                gif.still = Some(path);
                continue;
            }
            let slot = &mut gif.still;
            scope.spawn(move || {
                let fetched = ureq::get(&url)
                    .call()
                    .and_then(|mut response| response.body_mut().read_to_vec());
                if let Ok(bytes) = fetched
                    && std::fs::write(&path, bytes).is_ok()
                {
                    *slot = Some(path);
                }
            });
        }
    });
    Ok(gifs.into_iter().map(|(gif, _)| gif).collect())
}

/// Keeps a copy of a sent attachment where the view can show it, and
/// builds the archive row for it.
async fn file_outbound(
    client: &Client,
    chat: &str,
    me: &str,
    dir: &Path,
    mut prepared: Prepared,
    caption: Option<String>,
) -> Result<(Message, Vec<u8>), String> {
    if let Some(caption) = caption.filter(|caption| !caption.trim().is_empty()) {
        match &mut prepared.content {
            Content::Image { caption: slot, .. }
            | Content::Video { caption: slot, .. }
            | Content::Document { caption: slot, .. } => *slot = Some(caption.clone()),
            _ => {}
        }
        if let Some(image) = prepared.message.image_message.as_option_mut() {
            image.caption = Some(caption.clone());
        }
        if let Some(video) = prepared.message.video_message.as_option_mut() {
            video.caption = Some(caption.clone());
        }
        if let Some(document) = prepared.message.document_message.as_option_mut() {
            document.caption = Some(caption);
        }
    }
    let id = client.generate_message_id();
    let path = media_path(
        dir,
        chat,
        &id,
        &prepared.mime,
        prepared.file_name.as_deref(),
    );
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::write(&path, &prepared.bytes)
        .await
        .map_err(|error| error.to_string())?;
    let mut content = prepared.content;
    if let Some(media) = content.media_mut() {
        media.path = Some(path);
    }
    let row = Message {
        id,
        chat: chat.to_owned(),
        sender: me.to_owned(),
        sender_name: None,
        from_me: true,
        timestamp: crate::util::now(),
        content,
        status: Delivery::Pending,
        quoted: None,
        reactions: Vec::new(),
        edited: false,
        mentions: Vec::new(),
        forwarded: false,
        thumbnail: prepared.thumbnail,
    };
    Ok((row, prepared.message.encode_to_vec()))
}

/// Decodes one history chunk into rows, off the worker thread.
fn parse_history(compressed: &[u8]) -> Result<ParsedHistory, String> {
    let mut stream = HistorySyncStream::new(compressed, MAX_DECOMPRESSED);
    let mut chats = Vec::new();
    loop {
        let conversation = match stream.next_conversation() {
            Ok(Some(conversation)) => conversation,
            Ok(None) => break,
            Err(error) => return Err(error.to_string()),
        };
        chats.push(parse_conversation(conversation));
    }
    let remainder = stream.remainder().map_err(|error| error.to_string())?;
    let push_names = remainder
        .pushnames
        .iter()
        .filter_map(|entry| Some((entry.id.clone()?, entry.pushname.clone()?)))
        .collect();
    let lids = remainder
        .phone_number_to_lid_mappings
        .iter()
        .filter_map(|entry| Some((entry.lid_jid.clone()?, entry.pn_jid.clone()?)))
        .collect();
    Ok(ParsedHistory {
        chats,
        push_names,
        lids,
        stickers: remainder.recent_stickers,
    })
}

fn parse_conversation(conversation: wa::Conversation) -> ParsedChat {
    let mut messages = Vec::new();
    let mut revoked = Vec::new();
    let mut newest = 0;
    for entry in &conversation.messages {
        let Some(info) = entry.message.as_option() else {
            continue;
        };
        let Some(key) = info.key.as_option() else {
            continue;
        };
        let Some(id) = key.id.clone().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(message) = info.message.as_option() else {
            continue;
        };
        let from_me = key.from_me.unwrap_or(false);
        let timestamp = info.message_timestamp.unwrap_or(0) as i64;
        newest = newest.max(timestamp);
        let base = message.get_base_message();
        if let Some(protocol) = base.protocol_message.as_option() {
            if protocol.r#type == Some(wa::message::protocol_message::Type::REVOKE)
                && let Some(target) = protocol.key.as_option().and_then(|key| key.id.clone())
            {
                revoked.push(target);
            }
            continue;
        }
        if base.reaction_message.is_set() {
            continue;
        }
        let Some(content) = classify(base) else {
            continue;
        };
        let sender = info
            .participant
            .clone()
            .or_else(|| key.participant.clone())
            .filter(|sender| !sender.is_empty())
            .or_else(|| key.remote_jid.clone());
        use wa::web_message_info::Status;
        let mut status = if from_me {
            match info.status {
                Some(Status::READ) => Delivery::Read,
                Some(Status::PLAYED) => Delivery::Played,
                Some(Status::DELIVERY_ACK) => Delivery::Delivered,
                Some(Status::SERVER_ACK) => Delivery::Sent,
                Some(Status::PENDING) => Delivery::Pending,
                Some(Status::ERROR) => Delivery::Failed,
                _ => Delivery::Sent,
            }
        } else {
            Delivery::None
        };
        if from_me && status < Delivery::Read {
            if info
                .user_receipt
                .iter()
                .any(|receipt| receipt.read_timestamp.is_some())
            {
                status = Delivery::Read;
            } else if status < Delivery::Delivered
                && info
                    .user_receipt
                    .iter()
                    .any(|receipt| receipt.receipt_timestamp.is_some())
            {
                status = Delivery::Delivered;
            }
        }
        let quoted = context_of(base).and_then(|context| {
            let id = context.stanza_id.clone().filter(|id| !id.is_empty())?;
            Some(Quoted {
                mentions: Vec::new(),
                id,
                sender: context.participant.clone().unwrap_or_default(),
                sender_name: None,
                summary: context
                    .quoted_message
                    .as_option()
                    .and_then(|quoted| classify(quoted.get_base_message()))
                    .map(|content| content.summary())
                    .unwrap_or_default(),
            })
        });
        let reactions = info
            .reactions
            .iter()
            .filter_map(|reaction| {
                let text = reaction.text.clone().filter(|text| !text.is_empty())?;
                let key = reaction.key.as_option();
                let from_me = key.and_then(|key| key.from_me).unwrap_or(false);
                let who = key.and_then(|key| key.participant.clone());
                Some((who, from_me, text))
            })
            .collect();
        messages.push(ParsedMessage {
            id,
            sender,
            from_me,
            push_name: non_empty(&info.push_name),
            timestamp,
            content,
            status,
            quoted,
            reactions,
            mentions: mentioned_of(base),
            forwarded: forwarded_of(base),
            thumbnail: thumbnail_of(base),
            raw: message.encode_to_vec(),
        });
    }
    let last_activity = conversation
        .conversation_timestamp
        .or(conversation.last_msg_timestamp)
        .map(|timestamp| timestamp as i64)
        .unwrap_or(0)
        .max(newest);
    use wa::conversation::EndOfHistoryTransferType as End;
    let more_on_phone = conversation
        .end_of_history_transfer_type
        .map(|end| match end {
            End::COMPLETE_BUT_MORE_MESSAGES_REMAIN_ON_PRIMARY
            | End::COMPLETE_ON_DEMAND_SYNC_BUT_MORE_MSG_REMAIN_ON_PRIMARY => true,
            End::COMPLETE_AND_NO_MORE_MESSAGE_REMAIN_ON_PRIMARY
            | End::COMPLETE_ON_DEMAND_SYNC_WITH_MORE_MSG_ON_PRIMARY_BUT_NO_ACCESS => false,
        });
    ParsedChat {
        id: conversation.id.clone(),
        name: non_empty(&conversation.display_name).or_else(|| non_empty(&conversation.name)),
        unread: conversation.unread_count.unwrap_or(0),
        archived: conversation.archived.unwrap_or(false),
        pinned: conversation.pinned.unwrap_or(0) > 0,
        muted_until: conversation
            .mute_end_time
            .filter(|end| *end > 0)
            .map(|end| seconds(end as i64)),
        last_activity,
        pn_jid: conversation.pn_jid.clone(),
        lid_jid: conversation.lid_jid.clone(),
        more_on_phone,
        messages,
        revoked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_names_read_as_phones_or_ids() {
        assert_eq!(
            fallback_name("393331234567@s.whatsapp.net"),
            "+39 333 123 456 7"
        );
        assert_eq!(fallback_name("1-2@g.us"), "Group");
        assert_eq!(fallback_name("42@lid"), "42");
    }

    #[test]
    fn media_paths_keep_document_names_and_map_mimes() {
        let dir = Path::new("/cache");
        assert_eq!(
            media_path(dir, "1@s.whatsapp.net", "ABC", "image/jpeg", None),
            PathBuf::from("/cache/1_s_whatsapp_net-ABC.jpg")
        );
        assert_eq!(
            media_path(
                dir,
                "1@s.whatsapp.net",
                "ABC",
                "application/pdf",
                Some("tax return.pdf")
            ),
            PathBuf::from("/cache/ABC-tax_return.pdf")
        );
        assert_eq!(extension_for("audio/ogg; codecs=opus", None), "ogg");
        assert_eq!(extension_for("application/x-unknown", None), "x-unknown");
    }

    #[test]
    fn classification_covers_text_and_media() {
        let text = wa::Message::text("hello");
        assert_eq!(classify(&text), Some(Content::text("hello")));
        let image = wa::Message {
            image_message: whatsapp_rust::prelude::MessageField::some(wa::message::ImageMessage {
                caption: Some("look".into()),
                mimetype: Some("image/jpeg".into()),
                file_length: Some(10),
                width: Some(4),
                height: Some(3),
                jpeg_thumbnail: Some(vec![0xff, 0xd8]),
                ..Default::default()
            }),
            ..Default::default()
        };
        match classify(&image) {
            Some(Content::Image { caption, media }) => {
                assert_eq!(caption.as_deref(), Some("look"));
                assert_eq!(media.mime, "image/jpeg");
                assert_eq!((media.width, media.height), (Some(4), Some(3)));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(thumbnail_of(&image), Some(vec![0xff, 0xd8]));
        assert_eq!(classify(&wa::Message::default()), None);
    }

    #[test]
    fn link_previews_and_mentions_come_from_extended_text() {
        let message = wa::Message {
            extended_text_message: whatsapp_rust::prelude::MessageField::some(
                wa::message::ExtendedTextMessage {
                    text: Some("see fastpotify.rocks @123456@lid".into()),
                    matched_text: Some("https://fastpotify.rocks/".into()),
                    title: Some("fastpotify.rocks".into()),
                    description: Some("Spotify, native and fast".into()),
                    context_info: whatsapp_rust::prelude::MessageField::some(wa::ContextInfo {
                        mentioned_jid: vec!["123456@lid".into()],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        match classify(&message) {
            Some(Content::Text { preview, .. }) => {
                let preview = preview.expect("preview");
                assert_eq!(preview.url, "https://fastpotify.rocks/");
                assert_eq!(preview.title.as_deref(), Some("fastpotify.rocks"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(mentioned_of(&message), vec!["123456@lid".to_owned()]);
    }

    #[test]
    fn pictures_get_a_thumbnail_and_a_jpeg_body() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            300,
            200,
            image::Rgba([200, 30, 30, 255]),
        ));
        let jpeg = encode_jpeg(&image, 80).expect("encodes");
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        let thumbnail = thumbnail_jpeg(&image).expect("thumbnail");
        let small = image::load_from_memory(&thumbnail).expect("decodes");
        assert!(small.width() <= THUMBNAIL_SIDE && small.height() <= THUMBNAIL_SIDE);
    }

    #[test]
    fn millisecond_timestamps_are_normalised() {
        assert_eq!(seconds(1_700_000_000), 1_700_000_000);
        assert_eq!(seconds(1_700_000_000_000), 1_700_000_000);
        assert_eq!(seconds(-1), 0);
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use crate::model::{Content, Delivery, Message};

    const ME: &str = "15550001111@s.whatsapp.net";
    const PEER: &str = "4917663430455@s.whatsapp.net";
    const PEER_LID: &str = "167650256810092@lid";

    /// A worker with an in-memory archive and nothing on the other end of
    /// its channels; the receivers live as long as the worker.
    fn worker() -> (
        Worker,
        std::sync::mpsc::Receiver<Event>,
        mpsc::UnboundedReceiver<Command>,
        mpsc::UnboundedReceiver<Arc<wa_events::Event>>,
    ) {
        let (events, events_rx) = std::sync::mpsc::channel();
        let (commands, inbox) = mpsc::unbounded_channel();
        let (wa_sender, wa_events) = mpsc::unbounded_channel();
        let root =
            std::env::temp_dir().join(format!("fastsapp-worker-test-{}", std::process::id()));
        let worker = Worker {
            dirs: AppDirs::under(&root),
            events,
            commands,
            waker: Waker(Arc::new(std::sync::Mutex::new(None))),
            archive: Archive::in_memory().expect("archive"),
            client: None,
            handle: None,
            wa_sender,
            me_pn: Some(ME.to_owned()),
            me_lid: None,
            me_name: None,
            me_about: None,
            lid_to_pn: HashMap::new(),
            contacts: HashMap::new(),
            status: LinkStatus::Connected,
            pairing_phone: None,
            pair_code: None,
            qr: None,
            syncing: false,
            sync_deadline: None,
            group_info_requested: HashSet::new(),
            presence_subscribed: HashSet::new(),
            pending_older: HashMap::new(),
            older_warned: HashSet::new(),
            pending_avatars: HashMap::new(),
            sticker_fetches: HashSet::new(),
            sticker_downloads: HashSet::new(),
        };
        (worker, events_rx, inbox, wa_events)
    }

    fn own_message(id: &str, timestamp: i64) -> Message {
        Message {
            id: id.into(),
            chat: PEER.into(),
            sender: ME.into(),
            sender_name: None,
            from_me: true,
            timestamp,
            content: Content::text("hi"),
            status: Delivery::Sent,
            quoted: None,
            reactions: Vec::new(),
            edited: false,
            mentions: Vec::new(),
            forwarded: false,
            thumbnail: None,
        }
    }

    fn receipt(chat: &str, ids: &[&str], kind: ReceiptType) -> wa_events::Receipt {
        let chat: Jid = chat.parse().expect("jid");
        wa_events::Receipt::builder()
            .message_ids(ids.iter().map(|id| (*id).to_owned()).collect())
            .source(MessageSource {
                chat: chat.clone(),
                sender: chat,
                ..Default::default()
            })
            .timestamp(whatsapp_rust::wacore::time::now_utc())
            .r#type(kind)
            .offline(false)
            .build()
    }

    #[test]
    fn a_read_receipt_from_the_peers_privacy_id_moves_our_messages() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "R").expect("chat");
        for (id, when) in [("A1", 100), ("A2", 200), ("A3", 300)] {
            worker
                .archive
                .insert_message(&own_message(id, when), None)
                .expect("stored");
        }
        worker.learn_lid("167650256810092", "4917663430455");
        worker.on_receipt(&receipt(PEER_LID, &["A2"], ReceiptType::Read));
        let status = |id: &str| {
            worker
                .archive
                .message(PEER, id)
                .expect("read")
                .expect("row")
                .status
        };
        assert_eq!(status("A2"), Delivery::Read, "the named message");
        assert_eq!(status("A1"), Delivery::Read, "and everything before it");
        assert_eq!(status("A3"), Delivery::Sent, "not what came after");
    }

    #[test]
    fn inactive_counts_as_delivered_and_sender_only_in_the_chat_with_ourselves() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "R").expect("chat");
        worker.archive.ensure_chat(ME, "Me").expect("chat");
        worker
            .archive
            .insert_message(&own_message("C1", 100), None)
            .expect("stored");
        let mut to_self = own_message("S1", 100);
        to_self.chat = ME.into();
        worker
            .archive
            .insert_message(&to_self, None)
            .expect("stored");
        worker.on_receipt(&receipt(PEER, &["C1"], ReceiptType::Inactive));
        assert_eq!(
            worker
                .archive
                .message(PEER, "C1")
                .expect("read")
                .expect("row")
                .status,
            Delivery::Delivered,
            "an inactive device still received it"
        );
        worker.on_receipt(&receipt(PEER, &["C1"], ReceiptType::Sender));
        assert_eq!(
            worker
                .archive
                .message(PEER, "C1")
                .expect("read")
                .expect("row")
                .status,
            Delivery::Delivered,
            "our own other device says nothing about the peer"
        );
        worker.on_receipt(&receipt(ME, &["S1"], ReceiptType::Sender));
        assert_eq!(
            worker
                .archive
                .message(ME, "S1")
                .expect("read")
                .expect("row")
                .status,
            Delivery::Read,
            "a message to ourselves is read once the phone has it"
        );
    }

    #[test]
    fn a_delivery_receipt_from_the_phone_number_moves_only_the_named_message() {
        let (mut worker, _events, _inbox, _wa) = worker();
        worker.archive.ensure_chat(PEER, "R").expect("chat");
        for (id, when) in [("B1", 100), ("B2", 200)] {
            worker
                .archive
                .insert_message(&own_message(id, when), None)
                .expect("stored");
        }
        worker.on_receipt(&receipt(PEER, &["B2"], ReceiptType::Delivered));
        let status = |id: &str| {
            worker
                .archive
                .message(PEER, id)
                .expect("read")
                .expect("row")
                .status
        };
        assert_eq!(status("B2"), Delivery::Delivered);
        assert_eq!(status("B1"), Delivery::Sent);
    }
}
