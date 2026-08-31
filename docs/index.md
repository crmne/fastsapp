---
layout: home
title: FastsApp
description: A fast, native WhatsApp client for Linux, macOS, and Windows, written in Rust.
permalink: /
hero:
  name: FastsApp
  text: WhatsApp, native and fast
  tagline: A lightweight WhatsApp client with your full chats, voice messages, attachments, and notifications on Linux, macOS, and Windows.
  actions:
    - theme: brand
      text: Download
      link: /download/
    - theme: alt
      text: What is FastsApp?
      link: /what-is-fastsapp/
    - theme: alt
      text: GitHub
      link: https://github.com/crmne/fastsapp
  image:
    src: /screenshot.png
    alt: "FastsApp showing a chat with a photo, a document, a voice message, a quoted reply, and a link preview"
    width: 1894
    height: 1066

features:
  - icon: ⚡
    title: Lightweight
    details: A native binary with no embedded browser engine. It starts in well under a second and stays comfortable with years of chats.
  - icon: 🎤
    title: Voice messages
    details: They play where they are, waveform and all, and the send button records one when there is nothing typed. Opus in, Opus out, nothing to install.
  - icon: 🖼️
    title: Everything in place
    details: Photos, GIFs, stickers, documents, polls, locations, and link previews render in the conversation; attachments wait in the composer for a caption.
  - icon: 🔔
    title: Stays out of the way
    details: Closing the window keeps messages arriving from the tray, and notifications carry the sender's picture. Mute any chat and the phone follows.
  - icon: ⌨️
    title: Keyboard-first
    details: Search, chat switching, replies, and recording all have shortcuts; text anywhere can be swept and copied, WhatsApp's format included.
  - icon: 🔓
    title: Open source
    details: MIT-licensed Rust built with egui and whatsapp-rust. How it links to your phone is documented in full.
    link: https://github.com/crmne/fastsapp
    link_text: Read the source
---

<style>
  /* The hero image slot is sized for a square logo; the screenshot needs the
     room. Page-scoped overrides, so the theme stays untouched. */
  .VPHero .image-container {
    width: 100% !important;
    height: auto !important;
    transform: none !important;
  }
  .VPHero .image-src {
    position: relative !important;
    top: auto !important;
    left: auto !important;
    transform: none !important;
    width: 100% !important;
    height: auto !important;
    max-width: 100% !important;
    max-height: none !important;
    padding: 0 !important;
    border-radius: 12px;
    box-shadow: 0 12px 48px rgba(0, 0, 0, 0.45);
  }
  @media (max-width: 959px) {
    .VPHero .image {
      margin: 0 0 24px !important;
    }
  }
</style>
