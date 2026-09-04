# FastsApp

**WhatsApp, native and fast.** FastsApp is a WhatsApp client written in Rust
with [egui](https://github.com/emilk/egui). It uses
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust) for the WhatsApp Web
protocol. It links to your phone as a companion device, starts in well under a
second, and has no browser engine.

FastsApp is a sibling of [Fastpotify](https://github.com/crmne/fastpotify),
with the same native UI for a different service.

![FastsApp showing a chat with a photo, a document, a voice message, a quoted reply, and a link](docs/screenshot.png)

See **[fastsapp.rocks](https://fastsapp.rocks)** for downloads and guides.

![A group chat with sender names and pictures, a photo with reactions, a reply with a mention, and a poll](docs/screenshot-group.png)

![The linking screen with the QR code](docs/screenshot-link.png)

## What it does

- **Links to your phone.** Scan a QR code or link with your phone number.
  Recent history is copied to this computer after linking and stored here.
- **Chats.** See pinned, unread, muted, and archived chats, typing indicators,
  and message status. Search chats, saved messages, and contacts.
- **Conversations.** See replies, reactions, edits, deleted messages, read
  receipts, sender names, and group pictures. Older messages load as you
  scroll up, first from the local archive and then from your phone.
- **WhatsApp formatting.** Bold, italic, strikethrough, code, lists, quotes,
  mentions, and link previews are supported. Links are clickable. Emoji use
  the desktop's color emoji font, with a bundled fallback, and emoji-only
  messages are larger.
- **Send attachments with captions.** Paste a picture, drop files, or use the
  file picker. They stay in the composer until you send them or press Escape.
- **Mute chats** for eight hours, one week, or indefinitely. The setting also
  applies on your phone and to desktop notifications.
- **Voice messages.** Play, seek, record, reply with, and send voice messages
  in the chat. The app normalizes quiet recordings and handles OGG/Opus
  without external tools.
- **Send messages.** Press Enter to send text and Shift+Enter for a new line.
  You can swap these keys in Settings. The composer is focused when you open
  or return to a conversation; invoking search keeps focus in search, and
  Escape clears search and returns to the composer. Type `:name` to autocomplete
  an emoji without leaving the composer, or `@` in a group to mention a member.
  Reply, react, edit, forward, delete, and check when a message was sent,
  delivered, or read.
- **View attachments.** FastsApp downloads files up to 64 MB automatically or
  on click. Photos, stickers, GIFs, voice messages, audio, locations, contacts,
  polls, and link previews appear in the chat. Videos and documents open in
  their default desktop apps. If an attachment has expired, FastsApp asks your
  phone to upload it again.
- **Emoji, GIF, and sticker picker.** Search emoji and GIFs, use recent emoji
  and stickers, and save stickers with a right-click. Emoji autocomplete and
  picker search select their first match; use the arrow keys and Enter to
  choose it. GIF search needs a free GIPHY API key unless the build includes
  one.
- **Sticker packs.** Import a pack from a `signal.art` link or `.wastickers`
  file. Animated packs remain animated. Packs are stored as WebP files on your
  computer.
- **Consistent names.** Use names from your address book or public WhatsApp
  profile names across chats, replies, mentions, and notifications.
- **Groups.** See members, sender names, and sender pictures. Announcement
  groups are read-only for non-admins.
- **Presence.** See online, last-seen, and typing status, and send your typing
  status.
- **Runs in the background.** Closing the window keeps FastsApp linked in the
  system tray. Reopen it from the tray or by launching it again. Quit from the
  tray or with `Ctrl+Q`, or disable this behavior in Settings.
- **Desktop notifications.** Get notifications with the chat picture when you
  are away from the open chat. Muted chats do not notify you. On Linux,
  clicking a notification opens the chat.
- **Update notices.** FastsApp checks GitHub once a day and shows a download
  link when a newer release is available. You can turn this off in Settings.
- **Light and dark**, or follow the system. Zoom with Ctrl+plus and
  Ctrl+minus.
- **Copy text.** Select part of a message or copy across messages in
  WhatsApp's `[time, date] Name:` format. Contact names and numbers are also
  selectable.
- **Keyboard shortcuts.** `Ctrl+K` searches, `Alt+↑/↓` switches chats and
  keeps the active chat visible in the list, `Esc` cancels the current action,
  and `Ctrl+/` lists all shortcuts.
- **Local storage.** Messages are stored in one SQLite file and attachments
  in the cache directory. Unlinking deletes both and removes this device from
  your phone.

## What it does not do yet

- Play ordinary videos in the app (they open in your player), or reply to
  a message with an attachment.
- Calls, status posts, communities, newsletters, and group administration.

## Installing

On Arch Linux, FastsApp is in the AUR:

```sh
yay -S fastsapp-bin      # the released build, ready made
yay -S fastsapp          # the release, built from source
yay -S fastsapp-git      # built from the latest commit
```

Builds for every release are on the
[releases page](https://github.com/crmne/fastsapp/releases):

| Platform | File |
| --- | --- |
| Linux x86_64 and arm64 | `fastsapp-vX.Y.Z-<target>.tar.gz`, with the desktop file and icon in `packaging/` |
| Windows x64 and arm64 | `fastsapp-vX.Y.Z-<target>-setup.exe` (no administrator rights needed), or the `.zip` |
| macOS, universal | `fastsapp-vX.Y.Z-macos-universal.dmg` |

Unsigned macOS releases are not notarized. If macOS blocks the app, allow it
under **System Settings**, **Privacy & Security**.

### From source

FastsApp needs Rust. `rust-toolchain.toml` pins the exact version. On Linux,
it also needs GUI development packages:

```sh
# Debian and Ubuntu
sudo apt install libxkbcommon-dev libwayland-dev libgl1-mesa-dev
# Arch
sudo pacman -S libxkbcommon wayland mesa
```

Then:

```sh
cargo install --path .
fastsapp
```

The desktop file and icon are in `packaging/`.

`whatsapp-rust` is pinned to a Git commit because version 0.7.0 on crates.io
enables a `simd` feature that needs nightly Rust. The pinned commit builds on
stable Rust.

## Using it

On first start, scan the QR code from WhatsApp under **Linked devices**,
**Link a device**. To link without the camera, click **Link with phone number
instead**, enter your number with its country code, then enter the shown code
on your phone.

WhatsApp then sends your recent history. This can take a few minutes. A banner
shows the progress. New messages arrive live, and your phone does not need to
stay on the same network.

Right-click a chat or message to open its menu. Open Settings from the gear or
with `Ctrl+,`. Use the pencil to message a new number or save a contact. You
can also open a group member's contact card. Saved names sync through WhatsApp
to your phone and linked devices.

## Files

| What | Linux | Notes |
| --- | --- | --- |
| Settings | `~/.config/fastsapp/settings.json` | JSON, safe to edit |
| Device keys | `~/.local/state/fastsapp/session.db` | Owned by whatsapp-rust; deleting it unlinks |
| Messages | `~/.local/state/fastsapp/archive.db` | SQLite; raw messages contain the keys needed to download attachments |
| Attachments, avatars | `~/.cache/fastsapp/` | Safe to delete |
| Saved stickers and packs | `~/.local/state/fastsapp/stickers/` | Plain WebP files; each pack is a folder |
| Log of the last run | `~/.local/state/fastsapp/fastsapp.log` | `--verbose` for more |

macOS and Windows use the standard platform directories selected by the
`directories` crate. On first start, FastsApp moves data from its old
`fastwhatsapp` paths so the device remains linked.

## Developing

```sh
cargo run --features demo -- --demo            # sample chats, no connection
cargo run --features demo -- --demo-page login # or settings, pair, info, light, …
cargo run --features demo -- --demo-shot shot.png --demo-page chat,light
cargo test --all-features                      # includes a headless layout of every screen
cargo clippy --all-targets --all-features -- -D warnings
```

To include a default GIPHY key for GIF search, set it at build time. A key in
Settings overrides it:

```sh
FASTSAPP_GIPHY_KEY=your-key cargo build --release
```

`AGENTS.md` describes the architecture and the rules for changes.

## Disclaimer

FastsApp is an unofficial client and is not affiliated with WhatsApp or
Meta. Using an unofficial client may be against WhatsApp's terms of service
and could get an account suspended. Use it at your own risk.

## License

MIT. Inter and Noto Color Emoji are under the SIL Open Font License; the icons
are from [Lucide](https://lucide.dev) (ISC).
