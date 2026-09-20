# OmniDownloader

[![Release](https://img.shields.io/github/v/release/ASVLCII/OmniDownloader)](https://github.com/ASVLCII/OmniDownloader/releases/latest)
[![Build](https://github.com/ASVLCII/OmniDownloader/actions/workflows/ci.yml/badge.svg)](https://github.com/ASVLCII/OmniDownloader/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)

<p align="center">
  <img src="./omnidownloader-logo-shaded.svg" alt="OmniDownloader" width="900">
</p>

<p align="center">
  A terminal-first download manager with a persistent queue, background worker, and command-driven TUI.
</p>

![OmniDownloader TUI](./tui-preview.svg)

## Features

- Direct HTTP downloads with validated resume support and collision-safe output.
- Video, audio, and playlist downloads through yt-dlp.
- Media conversion through FFmpeg.
- Optional torrent and magnet support through aria2.
- Persistent SQLite queue shared by the TUI and scriptable CLI.
- Background worker with cancellation, recovery, and concurrent clients.
- Local cookie accounts, dependency diagnostics, and redacted logs.
- Keyboard and mouse interaction with fuzzy slash-command suggestions.

## Install

| Platform | Download |
|---|---|
| Windows x64 | `omnidownloader-windows-x86_64.exe` from the [latest release](https://github.com/ASVLCII/OmniDownloader/releases/latest) |
| Linux x64 / macOS | Attached to releases as CI publishes them; until then use Cargo below |

Rename the binary to `omnidownloader` (`omnidownloader.exe` on Windows) and place it in a directory on `PATH`.

Then open the TUI from any terminal:

```powershell
omnidownloader
```

### Cargo

```text
cargo install --git https://github.com/ASVLCII/OmniDownloader --locked
omnidownloader
```

## First download in one minute

1. Run `omnidownloader` and click the composer (or press `/`).
2. Paste a link and press Enter to preview it.
3. Press Enter again on `Add to queue`, or `Ctrl+Enter` to queue immediately.
4. Watch progress in the activity view and queue rail; finished files land in your download folder.

## Privacy

Everything runs on your computer: the queue, worker, accounts, and cookies stay in your local profile. There is no account, no telemetry, and logs redact URL secrets.

## TUI

Click the composer or press `/` to search commands. `Ctrl+P` searches everything: commands, downloads, library files, accounts, and system pages. Arrow keys move the highlight, Enter accepts it, Enter again runs it.

```text
/add       Download a URL, magnet, torrent, or list
/queue     View and control downloads
/library   Browse and export completed files
/convert   Convert a local media file
/account   Manage local cookie accounts
/system    Check tools, settings, plugins, and worker status
/help      Show commands and shortcuts
/quit      Exit safely
```

Pasting a URL opens a compact preview. Pasting a local media path opens conversion. `Ctrl+Enter` queues composer input immediately. The empty screen offers shortcuts for batch lists, torrent files, conversion, and add options. Selected downloads expose their own actions for keyboard and mouse. Missing tools surface one warning line; healthy systems stay quiet.

## CLI Examples

```powershell
omnidownloader download "https://example.com/file.zip" --direct --detach
omnidownloader info "https://example.com/video"
omnidownloader batch links.txt
omnidownloader queue list
omnidownloader queue pause <JOB_ID>
omnidownloader queue resume <JOB_ID>
omnidownloader auth import-cookies cookies.txt --name personal
omnidownloader convert clip.mov --format mp4 --detach
omnidownloader doctor
omnidownloader worker status
```

Add `--json` for newline-delimited machine-readable output. Use `--data-dir <profile>` to isolate application data.

## Optional Tools

OmniDownloader discovers configured tools through `PATH` and known executable locations. It never silently installs or updates them.

- [yt-dlp](https://github.com/yt-dlp/yt-dlp) for media sites and playlists
- [FFmpeg](https://ffmpeg.org/) for conversion
- [aria2](https://aria2.github.io/) for torrents and magnet links

Run `omnidownloader doctor` or `/system doctor` to inspect the current setup.

## Build

```powershell
$env:OMNIDOWNLOADER_WINDOWS_SDK = 'C:\path\to\Windows-SDK-Lib-version'
.\Build.ps1 test
.\Build.ps1 lint
.\Build.ps1 release
```

The Windows binary is written to `dist\omnidownloader.exe`. Standard Cargo builds are used on Linux and macOS.

## Verification

The controlled test harness covers formatting, Clippy with warnings denied, unit and engine tests, direct HTTP integrity, redirects, resume validation, destination collisions, worker recovery, concurrent clients, subprocess cleanup, plugin boundaries, yt-dlp against local media, and real Windows ConPTY keyboard/mouse interaction.

Live third-party providers, course adapters, browser callback authentication, and release packaging on every supported platform still require platform-specific validation.

## License

[MIT](./LICENSE)
