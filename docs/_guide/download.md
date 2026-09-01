---
title: Download
description: Get FastsApp for Linux, macOS, or Windows, with install instructions for each.
nav_order: 1
---

{% assign v = site.fastsapp_version %}
{% assign base = "https://github.com/crmne/fastsapp/releases/download/v" | append: v %}

The current version is **v{{ v }}**. SHA-256 checksums are in
[checksums.txt]({{ base }}/checksums.txt). Older versions are on the
[releases page](https://github.com/crmne/fastsapp/releases).

## Linux

On Arch Linux and derivatives, install from the
[AUR](https://aur.archlinux.org/packages/fastsapp-bin):

```sh
paru -S fastsapp-bin   # prebuilt
paru -S fastsapp       # builds from the release source
paru -S fastsapp-git   # builds from the latest commit
```

For other distributions, download a tarball with the binary, desktop file,
and icon:

- [fastsapp-v{{ v }}-x86_64-unknown-linux-gnu.tar.gz]({{ base }}/fastsapp-v{{ v }}-x86_64-unknown-linux-gnu.tar.gz)
- [fastsapp-v{{ v }}-aarch64-unknown-linux-gnu.tar.gz]({{ base }}/fastsapp-v{{ v }}-aarch64-unknown-linux-gnu.tar.gz)

FastsApp needs the standard egui libraries and ALSA:
`libglvnd`, `libxkbcommon`, `wayland`, `libx11`, and `alsa-lib` (on
Debian or Ubuntu: `libasound2`, `libgl1`, `libxkbcommon0`, `libwayland-client0`).
For color emoji, install `noto-fonts-emoji` (`fonts-noto-color-emoji` on
Debian). The file picker uses `xdg-desktop-portal`.

## macOS

One download for both Apple Silicon and Intel:

- [fastsapp-v{{ v }}-macos-universal.dmg]({{ base }}/fastsapp-v{{ v }}-macos-universal.dmg)

Open it and drag **FastsApp** to Applications.

### First open on macOS

This build is not notarized, so macOS blocks it the first time. Allow it in
Privacy & Security:

1. Double-click **FastsApp** in Applications. macOS says it cannot be
   opened because Apple cannot check it for malicious software. Click
   **Done**, not **Move to Trash**.
2. Open **System Settings**, then **Privacy & Security**.
3. Scroll down to the **Security** section, find *"FastsApp was blocked to
   protect your Mac"*, and click **Open Anyway**.
4. Authenticate, then click **Open Anyway** once more.

You can also clear the quarantine flag:

```sh
xattr -dr com.apple.quarantine /Applications/FastsApp.app
```

The `-r` also clears the flag from files inside the app bundle.

## Windows

The installer adds FastsApp to the Start menu and needs no administrator
rights. Choose x86_64 for most PCs or aarch64 for Windows on ARM:

- [fastsapp-v{{ v }}-x86_64-pc-windows-msvc-setup.exe]({{ base }}/fastsapp-v{{ v }}-x86_64-pc-windows-msvc-setup.exe)
- [fastsapp-v{{ v }}-aarch64-pc-windows-msvc-setup.exe]({{ base }}/fastsapp-v{{ v }}-aarch64-pc-windows-msvc-setup.exe)

To run FastsApp without installing it, download a zip, extract it, and run
`fastsapp.exe`.

- [fastsapp-v{{ v }}-x86_64-pc-windows-msvc.zip]({{ base }}/fastsapp-v{{ v }}-x86_64-pc-windows-msvc.zip)
- [fastsapp-v{{ v }}-aarch64-pc-windows-msvc.zip]({{ base }}/fastsapp-v{{ v }}-aarch64-pc-windows-msvc.zip)

SmartScreen may warn about an unknown publisher on first run. Choose **More
info**, then **Run anyway**.
