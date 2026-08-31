---
title: Getting Started
description: Install FastsApp, link it with your phone, and let the history arrive.
nav_order: 2
---

## Install

The [Download page](/download/) has packages and archives for Linux,
macOS, and Windows.

Or build from source with a recent stable [Rust](https://rustup.rs):

```sh
git clone https://github.com/crmne/fastsapp
cd fastsapp
cargo install --path .
```

On Linux the build needs the development packages any egui application
does, plus ALSA and cmake (libopus and the H.264 decoder compile from
source). On Arch:

```sh
sudo pacman -S --needed alsa-lib libxkbcommon wayland cmake
```

On Debian or Ubuntu:

```sh
sudo apt install build-essential cmake libasound2-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

A desktop entry ships in `packaging/applications/fastsapp.desktop`.

## Link with your phone

FastsApp is a companion device, like WhatsApp Web. Start it and either:

- scan the QR code with your phone (WhatsApp, **Settings**, **Linked
  devices**, **Link a device**), or
- click **Link with phone number** and type the eight-character code into
  the phone instead, when the camera is not handy.

The link survives restarts; your phone does not need to stay on the same
network, or online, for reading what has already arrived.

## The history arrives twice

Right after linking, the phone sends the recent history: the chat list
fills within seconds, and messages keep streaming in for a few minutes.
From then on, FastsApp keeps everything it sees in its own archive, so its
history grows past what WhatsApp replays. Scrolling to the top of a chat
asks your phone for what came before; the phone must be online to answer.

## A safe place to try things

Your own chat (the one WhatsApp calls **Message yourself**) behaves like
any other and is the place to try sending, reactions, edits, voice
messages, and attachments without an audience.
