# Fastsapp

**WhatsApp, native and fast.** A lightweight WhatsApp client written in Rust
with [egui](https://github.com/emilk/egui), speaking the WhatsApp Web protocol
through [whatsapp-rust](https://github.com/oxidezap/whatsapp-rust). It links
to your phone like WhatsApp Web does, starts in well under a second, and stays
small while it runs. There is no browser engine anywhere in the process.

Fastsapp is a sibling of [Fastpotify](https://github.com/crmne/fastpotify):
the same idea, the same look, a different service. (The name follows the
same pattern without carrying WhatsApp's trademark.)

![Fastsapp showing a chat with a photo, a document, a voice message, a quoted reply, and a link](docs/screenshot.png)

![A group chat with sender names and pictures, a photo with reactions, a reply with a mention, and a poll](docs/screenshot-group.png)

![The linking screen with the QR code](docs/screenshot-link.png)

## What it does

- **Links to your phone** by scanning a QR code, or by typing a code on the
  phone if you prefer to link with your phone number. Your chat history
  comes over once, right after linking, and is kept on this computer from
  then on.
- **Chats.** The list you know: pinned first, unread counts, muted and
  archived chats, who is typing, the last message with its ticks. Search
  by name, number, or the last message.
- **Conversations.** Messages in bubbles, grouped by day, with sender
  names, colours, and pictures in groups, quoted replies you can click to
  jump to, reactions, edits, deleted messages, and read receipts. Older
  messages load as you scroll up; when the archive runs out, the phone is
  asked for more.
- **Looks like WhatsApp.** `*bold*`, `_italic_`, `~struck~`, `` `code` ``,
  lists, and quotes render as they do on the phone; `@mentions` show names;
  links (bare domains and e-mail addresses too) are clickable, and the
  preview card WhatsApp attached to a link is shown. Emoji are drawn in
  colour from the desktop's emoji font (Noto Color Emoji on Linux, Apple
  Color Emoji on macOS), flags, skin tones, and families included; a
  message of nothing but emoji is shown large.
- **Attachments wait for a caption.** A pasted picture, dropped files, or
  files from the picker sit in the composer until you send, with whatever
  you typed as the caption; Escape drops them.
- **Mute.** Any chat, for eight hours, a week, or for good, from its menu
  or its info; the phone follows, and so do notifications.
- **Sends.** Text with Enter (Shift+Enter breaks a line; swap them in
  Settings), pictures pasted from the clipboard, and files picked with the
  paperclip or dropped on the window: pictures, videos, audio, and
  documents. Reply to a message from its menu, react with one of the quick
  emoji, edit or delete your own messages, and see when the other side is
  typing.
- **Attachments.** Everything up to 64 MB is fetched as it scrolls into
  view (or on a click, in Settings), with WhatsApp's blurred preview until
  then. Pictures show at WhatsApp's size, stickers on their own, GIFs and
  animated stickers play in place (GIFs are decoded in the app; `ffmpeg`
  is only tried for anything that is not H.264, when
  the desktop has one); other videos show their poster and length and,
  like voice messages and documents, open with whatever your desktop uses
  for them. Locations open in a map; contacts and polls are shown;
  forwarded messages say so.
- **Picker.** The smiley next to the composer opens emoji (searchable, by
  category, with your recent ones), stickers (the ones your phone used
  lately, synced when you link, plus those you have sent or received here),
  and GIF search through GIPHY (paste a free API key from
  developers.giphy.com in Settings, unless your build carries one).
- **One name per person.** From your address book, or as people call
  themselves on WhatsApp (a setting), and the same in the chat list,
  senders, replies, mentions, and notifications.
- **Groups.** Members under the group's name, the sender's picture beside
  each message (optionally in every chat), a click on either for their
  details, and the composer steps aside in announcement groups where only
  admins may post.
- **Presence.** Online and last-seen for the open chat, and typing
  indicators both ways.
- **Stays in the background.** Closing the window keeps Fastsapp linked
  and receiving in the system tray (a status notifier on Linux, the menu
  bar on macOS, the notification area on Windows); clicking the tray brings
  the window back. Quit from the tray menu or with `Ctrl+Q`; turn it off
  in Settings. A second launch shows the running window instead of
  starting a rival that would take over the link.
- **Notifies.** New messages come through the desktop's own notifications,
  with the person's or the group's picture, when the window is hidden, in
  the background, or on another chat; muted chats stay quiet; a click
  opens the chat (Linux). Off in Settings.
- **Light and dark**, or follow the system. Zoom with Ctrl+plus and
  Ctrl+minus.
- **Keyboard-first.** `Ctrl+K` searches, `Alt+↑/↓` walks the chats, `Esc`
  backs out of anything, `Ctrl+/` lists the rest.
- **Yours to keep.** Messages live in one SQLite file in your state
  directory; attachments in your cache directory. Unlinking from Settings
  forgets this device on the phone and deletes both.

## What it does not do yet

- Record voice messages, or play them in the app; play ordinary videos in
  the app (they open in your player).
- Calls, status posts, communities, newsletters, and group administration.
- Colour emoji on Windows: Segoe UI Emoji is not a bitmap font, so emoji
  stay monochrome there for now.
- Deleting or editing your own messages, and archiving or pinning a chat
  from here in a way the phone sees (those two are kept locally for now).

## Installing

On Arch Linux, Fastsapp is in the AUR:

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

The macOS app is not notarized unless the release was signed: if macOS
refuses to open it, right-click the app and choose Open once, or allow it
under System Settings, Privacy & Security.

### From source

Fastsapp needs a Rust toolchain (`rust-toolchain.toml` pins the exact
version) and, on Linux, the usual GUI development packages:

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

A `.desktop` file and an icon are in `packaging/` for a launcher entry.

`whatsapp-rust` is pinned to a git commit rather than the crates.io
release: 0.7.0 on crates.io enables a `simd` feature by default that needs a
nightly compiler, while the pinned commit builds on stable.

## Using it

The first start shows a QR code. On the phone, open WhatsApp, go to
**Linked devices**, tap **Link a device**, and point the camera at the
screen. If the camera is not an option, click *Link with phone number
instead*, type your number with its country code, and enter the code
Fastsapp shows on the phone.

WhatsApp then replays your recent history, which takes from a few seconds
to a couple of minutes depending on how much there is; a banner at the top
says when it is done. Everything after that arrives live, and the phone
does not need to stay on the same network.

Right-click a chat or a message for its menu. Settings are behind the gear
in the chat list, or `Ctrl+,`.

## Where it keeps things

| What | Linux | Notes |
| --- | --- | --- |
| Settings | `~/.config/fastsapp/settings.json` | Plain JSON, safe to edit |
| Device keys | `~/.local/state/fastsapp/session.db` | Owned by whatsapp-rust; deleting it unlinks |
| Messages | `~/.local/state/fastsapp/archive.db` | SQLite; the raw message keeps the keys to fetch its attachment later |
| Attachments, avatars | `~/.cache/fastsapp/` | Safe to delete |
| Log of the last run | `~/.local/state/fastsapp/fastsapp.log` | `--verbose` for more |

macOS and Windows use their platform's equivalents through the
`directories` crate. A setup from when the app was called fastwhatsapp is
moved to these paths the first time the renamed app starts, so the device
stays linked.

## Developing

```sh
cargo run --features demo -- --demo            # sample chats, no connection
cargo run --features demo -- --demo-page login # or settings, pair, info, light, …
cargo run --features demo -- --demo-shot shot.png --demo-page chat,light
cargo test --all-features                      # includes a headless layout of every screen
cargo clippy --all-targets --all-features -- -D warnings
```

To ship a build whose GIF search works without asking for a key, bake one
in at build time; it is never committed, and a key typed in Settings
still wins:

```sh
FASTSAPP_GIPHY_KEY=your-key cargo build --release
```

`AGENTS.md` describes the architecture and the rules for changes.

## Disclaimer

Fastsapp is an unofficial client and is not affiliated with WhatsApp or
Meta. Using an unofficial client may be against WhatsApp's terms of service
and could get an account suspended. Use it at your own risk.

## License

MIT. Inter is under the SIL Open Font License; the icons are from
[Lucide](https://lucide.dev) (ISC).
