# Fastsapp agent guide

Fastsapp is a small native WhatsApp client: Rust, egui, and the
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust) library for the
protocol. These notes are for coding agents and new contributors.

## Product boundaries

- Keep it a small native client. No browser engine, no telemetry, no
  hosted backend, no second account system.
- The protocol comes from whatsapp-rust. Do not reimplement pieces of it
  here, and do not advertise a capability merely because a protobuf field
  for it exists.
- Do not broaden a task into adjacent features or a general refactor.
  Preserve existing user behaviour unless the task changes it.

## Architecture

- `src/ui/` draws views and pushes `model::Action`s; `src/app.rs` applies
  them after the frame. Never mutate application state from inside a view
  beyond the view's own fields (composer text, search text, flags).
- `src/backend.rs` is the interface's handle to a tokio runtime on its own
  thread; `src/backend/worker.rs` runs there. It owns the whatsapp-rust
  `Bot`, the message archive, downloads, and profile pictures. The two
  sides talk only through `Command` (interface to runtime) and `Event`
  (runtime to interface); every event wakes the window through `Waker`.
- `src/archive.rs` is the SQLite store of chats, messages, contacts, and
  privacy-id mappings. WhatsApp replays history once, at link time, so the
  archive is the only copy. It keeps each message's raw protobuf because
  the keys to fetch an attachment live in it.
- `src/model.rs` holds the app's own types. Views never touch a protobuf;
  the worker translates in `classify()` and `parse_conversation()`.
- Chat ids are canonical strings: a chat behind a privacy id (`@lid`) is
  filed under its phone number once the mapping is known. Use
  `Worker::canonical` for anything that arrives as a `Jid`.
- `src/theme.rs` owns colours, fonts, and icons; `src/ui/widgets.rs` the
  shared controls. New icons go in `assets/icons/` as 24px Lucide-style SVGs
  and in the `icons!` table.
- `src/markup.rs` turns WhatsApp's text markup, links, and mentions into an
  egui `LayoutJob`; `src/emoji.rs` swaps every emoji for a placeholder
  glyph at layout time and paints the desktop's colour emoji bitmap over
  it afterwards (resolving sequences through the font's GSUB ligatures).
  Any text that can hold an emoji goes through `widgets::line` /
  `widgets::rich_text` or `markup::layout`, never a bare `Label`.
- `src/animation.rs` plays animated stickers and GIFs: WebP/GIF frames
  decode in-process, MP4 through the `ffmpeg` command; frames become
  textures on the interface thread and are dropped when unseen.
  `src/ui/picker.rs` is the emoji/GIF/sticker panel. GIF search uses the
  key from Settings, else one baked in at build time from
  `FASTSAPP_GIPHY_KEY` (`option_env!`); the repository carries none. The
  phone's recently used stickers arrive in `HistorySync.recent_stickers`
  when the device links and live in the archive's `stickers` table as raw
  `StickerMetadata`, fetched when the picker opens; favourite stickers sync
  through app state (`FavoriteSticker`), which whatsapp-rust does not
  surface, so they are not shown.
- `src/paths.rs` moves a setup left by the app's earlier name
  (`fastwhatsapp`) over once, so the linked device survives the rename.
- Older history comes from the phone on demand (`Command::FetchOlder` →
  `Client::fetch_message_history` → a `HistorySync` chunk with
  `sync_type == ON_DEMAND`); the archive is paged first, the phone only
  when it is exhausted.
- Platform-specific code belongs behind `cfg` blocks; a change for one
  platform must keep the other two compiling.

Three egui pitfalls this code has already hit:

- `consume_key(Modifiers::NONE, key)` also matches the key with Shift held
  (egui only insists on the modifiers you ask for), so the composer
  inspects the events itself to tell Enter from Shift+Enter.
- `with_layout(..., Align::Center)` directly inside a vertical container
  claims the whole available height; wrap it in `ui.horizontal`.
- `ui.horizontal` inside a right-aligned bubble lays out right to left;
  see `mirrored_row`. A bubble's own click target is registered before its
  contents (from last frame's rect) so links and quotes inside win clicks.
- `Popup::context_menu` opens on the *response's* right-click, which those
  inner widgets take for themselves; the bubble reads the right-click from
  the input over its own rect and opens `Popup::menu` itself, so the menu
  comes up anywhere on the message.

## Releasing

Bump `version` in `Cargo.toml`, commit, tag `vX.Y.Z`, and push the tag: the
release workflow builds every platform and publishes the GitHub release
with `checksums.txt`. Then update the AUR package in the maintainer's
`~/Code/aur/fastsapp` clone (new `pkgver`, `pkgrel=1`, the two Linux
checksums from `checksums.txt`, `makepkg --printsrcinfo > .SRCINFO`,
`makepkg -f` to prove it builds, commit, push). `fastsapp-git` only needs
touching when the build recipe or the dependencies change.

## Definition of done

- Add focused tests for changed behaviour. The `demo` feature carries sample
  data and a headless layout test of every screen (`src/demo.rs`); extend
  the sample when a new kind of content or state is added, and use
  `--demo-shot` to look at the result.
- Update the README when user-visible behaviour, settings, files, or network
  access changes.
- Run the full checks before finishing:

  ```sh
  cargo fmt --all --check
  cargo clippy --locked --all-targets -- -D warnings
  cargo clippy --locked --all-targets --all-features -- -D warnings
  cargo test --locked --all-targets
  cargo test --locked --all-targets --all-features
  RUSTDOCFLAGS='-D warnings' cargo doc --locked --all-features --no-deps
  ```

  Do not weaken a lint, delete a test, or add an `allow` merely to make
  them pass without explaining why the rule does not apply.
- Report platform coverage honestly: say what was run and what was only
  compiled.
- Never log message contents, phone numbers, keys, or QR payloads at a
  level that ships. The log file is meant to be attached to bug reports.
