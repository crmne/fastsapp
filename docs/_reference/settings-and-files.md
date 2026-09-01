---
title: Settings & Files
description: Settings and paths for the archive, configuration, caches, and logs.
nav_order: 0
---

## File locations

FastsApp follows each platform's conventions. On Linux:

| What | Where | Safe to delete? |
| --- | --- | --- |
| Settings | `~/.config/fastsapp/settings.json` | Yes, you lose preferences |
| Message archive | `~/.local/state/fastsapp/archive.db` | Yes; only history available from WhatsApp can be restored |
| Session keys | `~/.local/state/fastsapp/session.db` | Yes; you must link again |
| Attachments | `~/.cache/fastsapp/media/` | Yes; available files download again when viewed |
| Profile pictures | `~/.cache/fastsapp/avatars/` | Always |
| Stickers | `~/.cache/fastsapp/stickers/` | Always |
| GIF search stills | `~/.cache/fastsapp/gifs/` | Always |
| Last run's log | `~/.local/state/fastsapp/fastsapp.log` | Always |
| Crash log | `~/.local/state/fastsapp/panic.log` | Always |

Back up the archive if you need its history. WhatsApp sends only recent
history to a new device, although FastsApp can request some older messages from
the phone. Clearing the media cache makes FastsApp download attachments again.
Expired attachments may still be available through the phone.

On macOS, settings, state, and the logs are in
`~/Library/Application Support/me.paolino.fastsapp` and the caches in
`~/Library/Caches/me.paolino.fastsapp`. On Windows, settings are in
`%APPDATA%\paolino\fastsapp\config`, state and the logs in
`%LOCALAPPDATA%\paolino\fastsapp\data`, and the caches in
`%LOCALAPPDATA%\paolino\fastsapp\cache`.

## Settings

Changes on the Settings page are saved to `settings.json` immediately:

- **Theme**: light, dark, or follow the system.
- **Enter sends**: swap Enter and Shift+Enter.
- **Download attachments automatically**: download files up to 64 MB when
  they enter view, or only when clicked.
- **Show sender pictures**: avatars next to group messages.
- **Names from your address book**: use contact names everywhere. When off,
  prefer public profile names.
- **Send read receipts**: the blue ticks others see.
- **Keep running in the background**: keep FastsApp in the tray when the
  window closes.
- **Notifications**: use desktop notifications with the chat picture.
- **GIPHY API key**: required for GIF search unless the build includes one.
  Set `FASTSAPP_GIPHY_KEY` at compile time to include a default key.

## The log

Each run replaces `fastsapp.log` and records warnings and errors. Include the
end of this file when reporting an issue.
