# OmniDownloader

OmniDownloader is an independently implemented Rust download manager. It is a separate project and does not fork or depend on OmniGet.

## Current scope

The starter milestone includes:

- A shared asynchronous engine and persistent SQLite queue.
- Scriptable commands for downloads, inspection, batches, queue control, cookies, conversion, tools, and dependency checks.
- A Ratatui/Crossterm dashboard entry point.
- A local worker process with versioned JSON IPC.
- Direct HTTP downloads, yt-dlp media jobs, FFmpeg conversion, and optional aria2 torrent jobs.
- Redacted diagnostics and isolated application data under the selected profile.

Provider-specific authentication, broad course support, plugins, torrent behavior, and the full interactive dashboard still need deeper implementation and live validation.

## Build on Windows

`Build.ps1` keeps Cargo build output on the spacious build drive and configures the Visual Studio toolchain used by this checkout:

```powershell
$env:OMNIDOWNLOADER_WINDOWS_SDK = 'Z:\BugSeed\.build\windows-sdk'
.\Build.ps1 build
.\Build.ps1 test
.\Build.ps1 lint
```

The binary is copied to `dist\omnidownloader.exe`.

## Commands

```text
omnidownloader
omnidownloader download <url>
omnidownloader info <url>
omnidownloader batch <file>
omnidownloader queue list|pause|resume|cancel|retry
omnidownloader auth import-cookies <file>
omnidownloader doctor
omnidownloader worker status|stop
```

Add `--json` for newline-delimited machine-readable output. Use `--data-dir <profile>` to keep a test profile separate from the default data directory.

## Dependency discovery

The application discovers configured tools through `PATH` and known executable locations. `doctor` reports the exact executable path or setup guidance. It does not install or update tools.

## Verification boundary

The current verified checks are Windows build, formatting, Clippy with warnings denied, persistent-store tests, queue/worker CLI smoke tests, and dependency discovery. macOS/Linux builds, live provider downloads, terminal rendering, interruption/resume behavior, and background-worker failure recovery remain unverified.
