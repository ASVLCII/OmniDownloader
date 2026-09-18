# Astra handoff

This checkout is prepared as an independent OmniDownloader project. Keep the project separate from OmniGet and preserve the public protocol types in `crates/omnidownloader/src/protocol.rs` as the compatibility boundary.

## Verified on 2026-09-16

- `cargo fmt --all -- --check` passes.
- `Build.ps1 test` passes: 11 engine tests, unit targets, and doc tests.
- `Build.ps1 lint` passes with `-D warnings`.
- `Build.ps1 build` produces `dist\omnidownloader.exe`.
- CLI smoke: `queue list`, `worker status`, `worker stop`, and post-stop status work in a fresh profile.
- `doctor --json` reports yt-dlp, FFmpeg, and FFprobe on this machine and gives guidance for missing optional tools.

## Deliberately deferred

The following areas need a careful design and integration pass rather than small fixes:

- Full TUI hit testing, focus behavior, resizing, mouse interaction, dialogs, and visual regression coverage.
- Worker scheduling, cancellation, restart recovery, simultaneous clients, and cross-platform IPC/security.
- Direct HTTP range handling, redirects, duplicate destinations, write failures, and retry semantics.
- yt-dlp progress/playlist parsing and live media validation.
- Course adapters, browser callback authentication, plugin execution, and torrent support.
- macOS and Linux builds plus release packaging.

Do not describe those areas as complete until they have platform-appropriate tests or controlled live evidence.
