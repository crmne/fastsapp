---
title: What is FastsApp?
description: Why FastsApp exists, what it supports, and its current limitations.
nav_order: 0
---

## Why FastsApp

WhatsApp has no official Linux app, and its web client lives in a browser
tab. FastsApp is a small graphical client instead: a native WhatsApp app
written in Rust with [egui](https://github.com/emilk/egui), speaking to
WhatsApp through
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust). It is a single
native binary with no embedded browser engine, starts in well under a
second, and keeps its layout close to WhatsApp Web, so nothing needs
relearning.

![FastsApp showing a chat with a photo, a voice message, and a link preview](/screenshot.png)

## What it does

- **Your chats, kept.** FastsApp links to your phone as a companion device,
  like WhatsApp Web. Messages are stored in one SQLite file on your disk,
  so history survives restarts and grows past what WhatsApp replays. Older
  history is fetched from the phone as you scroll.
- **Sends everything usual.** Text with formatting, replies, edits,
  reactions, pictures pasted or dropped, files, stickers, GIFs, and voice
  messages recorded in the app. Attachments wait in the composer so a
  caption can join them.
- **Plays everything usual.** Voice messages play in the bubble with
  WhatsApp's own waveform; GIFs and animated stickers play in place, with
  the video and audio decoders built into the binary, so nothing needs
  installing.
- **One name per person.** Names come from your address book by default,
  the way your phone shows them, in chats, mentions, replies, and
  notifications alike; a setting switches to public names.
- **Stays out of the way.** Closing the window keeps messages arriving
  from the system tray. Notifications show the person's or group's
  picture, a click opens the chat, and muting a chat here mutes it on the
  phone too.
- **Text you can take.** Sweep any part of a message and copy it; a
  selection across messages copies the way the phone shares one, each line
  stamped with the time, the date, and the writer.

## What it does not do yet

FastsApp deliberately has a limited scope:

- Calls, status posts, communities, newsletters, and group administration.
- Playing ordinary videos in the app; they open in your player. Voice
  messages and GIFs do play in place.
- Replying with an attachment (replying with text or a voice message
  works).
- Colour emoji on Windows: Segoe UI Emoji is not a bitmap font, so emoji
  stay monochrome there for now.

If something misbehaves, [an issue](https://github.com/crmne/fastsapp/issues)
should say what happened, what you expected, and roughly when, so the log
in your state directory has it.

## Account safety

FastsApp is an **unofficial** client, and WhatsApp's terms of service do
not endorse unofficial clients. FastsApp keeps its behaviour as close to
WhatsApp Web as it can: it links as a companion device over WhatsApp's own
multi-device protocol, sends the same receipts a browser tab would, does
not automate messages, and does nothing in bulk. Even so, use it with the
understanding that WhatsApp could object; if that risk is not acceptable
to you, or the account is critical, stay with the official clients.

## Prior art

FastsApp speaks WhatsApp through
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust), which grew out
of the [whatsmeow](https://github.com/tulir/whatsmeow) lineage. WhatsApp
Web defines the companion-device model it follows.
[Fastpotify](https://fastpotify.rocks) is its sibling, the same idea
applied to Spotify.

FastsApp is an independent project, not affiliated with or endorsed by
WhatsApp LLC or Meta. WhatsApp is a trademark of WhatsApp LLC.
