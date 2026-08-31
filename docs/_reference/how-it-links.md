---
title: How it links
description: The companion-device model, what the phone is needed for, and what stays local.
nav_order: 1
---

## A companion device

Fastsapp registers with WhatsApp as a linked device over the multi-device
protocol, exactly the slot WhatsApp Web and WhatsApp Desktop use, through
[whatsapp-rust](https://github.com/oxidezap/whatsapp-rust). Messages are
end-to-end encrypted to it like to any device; the pairing happens through
the QR code or the pairing code, and the phone lists it under **Linked
devices**, where it can be signed out at any time.

One connection exists per linked device: running Fastsapp twice, or
Fastsapp and another client on the same slot, has them taking the
connection from each other. Fastsapp guards against its own copies (a
second launch just shows the window of the first), and links on its own
slot otherwise.

## What the phone is for

- **History.** WhatsApp replays only recent history to a fresh device.
  Fastsapp archives everything it sees from then on, and asks the phone
  for older messages when you scroll past the archive; the phone must be
  online to answer those requests, and to re-upload attachments whose
  server copies expired.
- **Nothing else.** Sending, receiving, receipts, and presence go through
  WhatsApp's servers directly; the phone can be offline for all of it.

## What stays local

The message archive, the media, and the session keys live on your disk
and never leave it; [Settings & Files](/settings-and-files/) lists every
path. Fastsapp talks to WhatsApp's servers, and to GIPHY only when GIF
search is used with an API key. There is no telemetry.

Unlinking from Settings tells the phone to forget the device and deletes
the local archive and caches.
