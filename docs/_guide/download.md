---
title: Download
description: Get FastsApp for Linux, macOS, or Windows, with install instructions for each.
nav_order: 1
---

{% assign v = site.fastsapp_version %}
{% assign base = "https://github.com/crmne/fastsapp/releases/download/v" | append: v %}

The current version is **v{{ v }}**. Every file below, with its SHA-256, is
listed in [checksums.txt]({{ base }}/checksums.txt); all versions live on
the [releases page](https://github.com/crmne/fastsapp/releases).

## Linux

On Arch and derivatives, from the [AUR](https://aur.archlinux.org/packages/fastsapp-bin):

```sh
paru -S fastsapp-bin   # prebuilt
paru -S fastsapp       # builds from the release source
paru -S fastsapp-git   # builds from the latest commit
```

Elsewhere, a tarball with the binary, the desktop entry, and the icon:

- [fastsapp-v{{ v }}-x86_64-unknown-linux-gnu.tar.gz]({{ base }}/fastsapp-v{{ v }}-x86_64-unknown-linux-gnu.tar.gz)
- [fastsapp-v{{ v }}-aarch64-unknown-linux-gnu.tar.gz]({{ base }}/fastsapp-v{{ v }}-aarch64-unknown-linux-gnu.tar.gz)

It needs the libraries any egui app does, plus ALSA:
`libglvnd`, `libxkbcommon`, `wayland`, `libx11`, and `alsa-lib` (on
Debian or Ubuntu: `libasound2`, `libgl1`, `libxkbcommon0`, `libwayland-client0`).
For colour emoji install `noto-fonts-emoji` (`fonts-noto-color-emoji` on
Debian); the file picker uses `xdg-desktop-portal`.

## macOS

One download for both Apple Silicon and Intel:

- [fastsapp-v{{ v }}-macos-universal.dmg]({{ base }}/fastsapp-v{{ v }}-macos-universal.dmg)

Open it and drag **FastsApp** to Applications.

### First open on macOS

This build is not yet notarized with Apple, so macOS blocks it the first
time. Recent macOS versions no longer let you bypass this with a
right-click, so you open it once through Privacy & Security:

1. Double-click **FastsApp** in Applications. macOS says it cannot be
   opened because Apple cannot check it for malicious software. Click
   **Done** (do **not** click Move to Trash).
2. Open **System Settings**, then **Privacy & Security**.
3. Scroll down to the **Security** section, find *"FastsApp was blocked to
   protect your Mac"*, and click **Open Anyway**.
4. Authenticate, then click **Open Anyway** once more.

Or clear the quarantine flag instead:

```sh
xattr -dr com.apple.quarantine /Applications/FastsApp.app
```

The `-r` matters: it clears the flag from the files inside the bundle too.

## Windows

The installer adds FastsApp to the Start menu and needs no administrator
rights. Choose x86_64 for most PCs or aarch64 for Windows on ARM:

- [fastsapp-v{{ v }}-x86_64-pc-windows-msvc-setup.exe]({{ base }}/fastsapp-v{{ v }}-x86_64-pc-windows-msvc-setup.exe)
- [fastsapp-v{{ v }}-aarch64-pc-windows-msvc-setup.exe]({{ base }}/fastsapp-v{{ v }}-aarch64-pc-windows-msvc-setup.exe)

If you would rather not install anything, the same program comes as a zip:
unpack it and run `fastsapp.exe`.

- [fastsapp-v{{ v }}-x86_64-pc-windows-msvc.zip]({{ base }}/fastsapp-v{{ v }}-x86_64-pc-windows-msvc.zip)
- [fastsapp-v{{ v }}-aarch64-pc-windows-msvc.zip]({{ base }}/fastsapp-v{{ v }}-aarch64-pc-windows-msvc.zip)

Either way, SmartScreen may warn about an unknown publisher on first run;
choose More info, then Run anyway.
