---
title: Settings & Files
description: Where Fastsapp keeps configuration, the archive, and caches, and what is safe to delete.
nav_order: 0
---

## Where things live

Fastsapp follows each platform's conventions. On Linux:

| What | Where | Safe to delete? |
| --- | --- | --- |
| Settings | `~/.config/fastsapp/settings.json` | Yes, you lose preferences |
| Message archive | `~/.local/state/fastsapp/archive.db` | Yes, and chats start over from what WhatsApp replays |
| Session keys | `~/.local/state/fastsapp/session.db` | Yes, you link again |
| Attachments | `~/.cache/fastsapp/media/` | Yes, each is fetched again when looked at, if the servers still hold it |
| Profile pictures | `~/.cache/fastsapp/avatars/` | Always |
| Stickers | `~/.cache/fastsapp/stickers/` | Always |
| GIF search stills | `~/.cache/fastsapp/gifs/` | Always |
| Last run's log | `~/.local/state/fastsapp/fastsapp.log` | Always |
| Crash log | `~/.local/state/fastsapp/panic.log` | Always |

The archive is the one file worth backing up: WhatsApp replays only
recent history to a fresh device, so a deleted archive means older
messages come back only as far as the phone answers scroll-back requests.
Attachments that WhatsApp's servers have expired can usually be fetched
again through the phone, but a cleared media cache still means fetching.

On macOS, settings, state, and the logs are in
`~/Library/Application Support/me.paolino.fastsapp` and the caches in
`~/Library/Caches/me.paolino.fastsapp`. On Windows, settings are in
`%APPDATA%\paolino\fastsapp\config`, state and the logs in
`%LOCALAPPDATA%\paolino\fastsapp\data`, and the caches in
`%LOCALAPPDATA%\paolino\fastsapp\cache`.

## Settings

Everything on the Settings page writes `settings.json` as it changes:

- **Theme**: light, dark, or follow the system.
- **Enter sends**: swap Enter and Shift+Enter.
- **Fetch attachments automatically**: up to 64 MB as they scroll into
  view, or only on a click.
- **Show sender pictures**: avatars next to group messages.
- **Names from your address book**: how people are named everywhere; off
  means public profile names.
- **Send read receipts**: the blue ticks others see.
- **Keep running in the background**: the tray behaviour on close.
- **Notifications**: with the sender's picture, per the system's style.
- **GIPHY API key**: GIF search asks GIPHY directly; without a key the
  GIF tab explains where to get one. Builds can bake a key in with the
  `FASTSAPP_GIPHY_KEY` environment variable at compile time.

## The log

Each run writes `fastsapp.log` fresh, warnings and errors only. When
reporting an issue, the tail of that file usually says what went wrong.
