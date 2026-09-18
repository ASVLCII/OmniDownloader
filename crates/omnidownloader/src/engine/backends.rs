use super::{now, paths::Paths, redact, safe_filename, worker::SharedStore};
use crate::protocol::*;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

pub fn find_tool(settings: &Settings, name: &str) -> Option<PathBuf> {
    if let Some(path) = settings.tools.get(name) {
        return path.is_file().then(|| path.clone());
    }
    which::which(name).ok()
}
pub fn doctor(settings: &Settings) -> Vec<Dependency> {
    [
        (
            "yt-dlp",
            "Video, audio, playlists and supported course sources",
            "Install yt-dlp: https://github.com/yt-dlp/yt-dlp#installation",
        ),
        (
            "ffmpeg",
            "Merge streams and convert media",
            "Install FFmpeg: https://ffmpeg.org/download.html",
        ),
        (
            "ffprobe",
            "Inspect local media",
            "Included with FFmpeg: https://ffmpeg.org/download.html",
        ),
        (
            "aria2c",
            "Optional torrents and magnet links",
            "Install aria2: https://github.com/aria2/aria2/releases",
        ),
        (
            "deno",
            "Optional JavaScript runtime for yt-dlp extractors",
            "Install Deno: https://docs.deno.com/runtime/getting_started/installation/",
        ),
    ]
    .into_iter()
    .map(|(name, purpose, guidance)| Dependency {
        name: name.into(),
        path: find_tool(settings, name),
        purpose: purpose.into(),
        guidance: guidance.into(),
    })
    .collect()
}
fn require(settings: &Settings, name: &str) -> Result<PathBuf> {
    find_tool(settings, name).with_context(|| {
        format!("{name} is missing. Run omnidownloader doctor for setup guidance.")
    })
}
pub fn is_direct(spec: &JobSpec) -> bool {
    if spec.options.direct {
        return true;
    }
    url::Url::parse(&spec.source)
        .ok()
        .and_then(|u| {
            Path::new(u.path())
                .extension()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
        })
        .is_some_and(|ext| {
            [
                "zip", "7z", "tar", "gz", "exe", "msi", "pdf", "epub", "mp4", "mp3", "m4a", "mkv",
                "webm", "flac", "wav", "png", "jpg", "jpeg", "gif", "iso", "bin", "txt", "csv",
            ]
            .contains(&ext.as_str())
        })
}
fn http_client(spec: &JobSpec) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(45))
        .user_agent("OmniDownloader/0.1");
    if let Some(ref proxy) = spec.options.proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    Ok(builder.build()?)
}
fn ytdlp_base(spec: &JobSpec, paths: &Paths, settings: &Settings) -> Result<Command> {
    let mut command = Command::new(require(settings, "yt-dlp")?);
    command.args([
        "--ignore-config",
        "--no-cache-dir",
        "--no-colors",
        "--socket-timeout",
        "30",
        "--retries",
        "3",
        "--fragment-retries",
        "3",
    ]);
    if let Some(ref proxy) = spec.options.proxy {
        command.arg("--proxy").arg(proxy);
    }
    if let Some(ref account) = spec.options.account {
        let file = paths.account_file(account)?;
        anyhow::ensure!(
            file.is_file(),
            "Account {account} is missing; import cookies first"
        );
        command.arg("--cookies").arg(file);
    }
    if let Some(path) = find_tool(settings, "ffmpeg") {
        command.arg("--ffmpeg-location").arg(path);
    }
    if let Some(path) = find_tool(settings, "deno") {
        command
            .arg("--js-runtimes")
            .arg(format!("deno:{}", path.display()));
    }
    Ok(command)
}
pub async fn inspect(spec: &JobSpec, paths: &Paths, settings: &Settings) -> Result<MediaInfo> {
    if let JobKind::Plugin(ref id) = spec.kind {
        return super::plugins::inspect(paths, id, spec).await;
    }
    if spec.kind == JobKind::Convert {
        let mut cmd = Command::new(require(settings, "ffprobe")?);
        cmd.args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(&spec.source);
        let output = capture(cmd, Duration::from_secs(30)).await?;
        let v: serde_json::Value = serde_json::from_slice(&output)?;
        return Ok(MediaInfo {
            title: Path::new(&spec.source)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            source: spec.source.clone(),
            duration: v["format"]["duration"]
                .as_str()
                .and_then(|s| s.parse().ok()),
            size: v["format"]["size"].as_str().and_then(|s| s.parse().ok()),
            formats: vec!["mp4".into(), "mp3".into(), "flac".into(), "wav".into()],
            entries: vec![],
        });
    }
    if is_direct(spec) {
        let response = http_client(spec)?
            .head(&spec.source)
            .send()
            .await?
            .error_for_status()?;
        let title = spec
            .options
            .filename
            .clone()
            .unwrap_or_else(|| filename_from_url(&spec.source));
        return Ok(MediaInfo {
            title,
            source: spec.source.clone(),
            // A HEAD response has an empty body; its header describes the GET size.
            size: response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok()),
            formats: vec![response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("file")
                .into()],
            ..Default::default()
        });
    }
    let mut cmd = ytdlp_base(spec, paths, settings)?;
    cmd.args([
        "--dump-single-json",
        "--flat-playlist",
        "--playlist-end",
        "500",
        "--",
    ])
    .arg(&spec.source);
    let output = capture(cmd, Duration::from_secs(90)).await?;
    let v: serde_json::Value = serde_json::from_slice(&output)?;
    let entries = v["entries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .enumerate()
                .map(|(i, e)| MediaEntry {
                    index: i + 1,
                    title: e["title"].as_str().unwrap_or("Untitled").into(),
                    source: e["webpage_url"]
                        .as_str()
                        .or(e["url"].as_str())
                        .unwrap_or("")
                        .into(),
                })
                .collect()
        })
        .unwrap_or_default();
    let formats = v["formats"]
        .as_array()
        .map(|formats| {
            formats
                .iter()
                .map(|f| {
                    format!(
                        "{} · {} · {}",
                        f["format_id"].as_str().unwrap_or("?"),
                        f["resolution"].as_str().unwrap_or("audio"),
                        f["ext"].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(MediaInfo {
        title: v["title"].as_str().unwrap_or("Untitled").into(),
        source: spec.source.clone(),
        duration: v["duration"].as_f64(),
        size: v["filesize"].as_u64(),
        formats,
        entries,
    })
}

pub async fn execute(
    job: Job,
    paths: &Paths,
    settings: &Settings,
    store: SharedStore,
    cancel: CancellationToken,
) -> Result<Vec<PathBuf>> {
    tokio::fs::create_dir_all(&job.destination)
        .await
        .context("Cannot create destination directory")?;
    match &job.spec.kind {
        JobKind::Convert => convert(job, settings, store, cancel).await,
        JobKind::Torrent => torrent(job, settings, store, cancel).await,
        JobKind::Plugin(id) => super::plugins::execute(paths, id, &job, store, cancel).await,
        JobKind::Download if is_direct(&job.spec) => direct(job, store, cancel).await,
        _ => ytdlp(job, paths, settings, store, cancel).await,
    }
}
pub fn update(store: &SharedStore, id: &str, f: impl FnOnce(&mut Job)) -> Result<()> {
    let store = store.lock().unwrap();
    let mut job = store.get(id)?;
    if job.status == Status::Active {
        f(&mut job);
        job.updated = now();
        store.put(&job)?;
    }
    Ok(())
}
fn filename_from_url(source: &str) -> String {
    url::Url::parse(source)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(safe_filename))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "download.bin".into())
}
async fn direct(job: Job, store: SharedStore, cancel: CancellationToken) -> Result<Vec<PathBuf>> {
    let name = job
        .spec
        .options
        .filename
        .clone()
        .unwrap_or_else(|| filename_from_url(&job.spec.source));
    let final_path = job.destination.join(&name);
    let partial = job.destination.join(format!("{name}.part"));
    anyhow::ensure!(
        !final_path.exists(),
        "Destination already exists; completed files are never overwritten"
    );
    if !job.resume_supported && job.downloaded == 0 && partial.exists() {
        tokio::fs::rename(
            &partial,
            job.destination.join(format!(
                "{name}.preserved-{}",
                uuid::Uuid::new_v4().simple()
            )),
        )
        .await?;
    }
    let offset = tokio::fs::metadata(&partial)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut request = http_client(&job.spec)?
        .get(&job.spec.source)
        .header("Accept-Encoding", "identity");
    if offset > 0 {
        request = request.header("Range", format!("bytes={offset}-"));
        if let Some(validator) = job.etag.as_ref().or(job.last_modified.as_ref()) {
            request = request.header("If-Range", validator);
        } else {
            update(&store, &job.id, |j| j.resume_supported = false)?;
            anyhow::bail!("This server supplied no resume validator. Restart requires confirmation; the partial file will be preserved.");
        }
    }
    let response = tokio::select! { _=cancel.cancelled()=>anyhow::bail!("Paused"), r=request.send()=>r.context("HTTP connection failed")? };
    if offset > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        update(&store, &job.id, |j| j.resume_supported = false)?;
        anyhow::bail!("Server cannot safely continue this partial file. Confirm restart; the existing partial file will be preserved.");
    }
    let response = response.error_for_status()?;
    let total = if offset > 0 {
        let validation = validate_resume_response(&response, &job, offset);
        if validation.is_err() {
            update(&store, &job.id, |j| j.resume_supported = false)?;
        }
        Some(validation?)
    } else {
        anyhow::ensure!(
            response.status() != reqwest::StatusCode::PARTIAL_CONTENT,
            "Server returned an unsolicited partial response; no file was written"
        );
        response.content_length()
    };
    let etag = response
        .headers()
        .get("etag")
        .and_then(|h| h.to_str().ok())
        .filter(|s| !s.starts_with("W/"))
        .map(str::to_owned);
    let modified = response
        .headers()
        .get("last-modified")
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned);
    update(&store, &job.id, |j| {
        j.title = name.clone();
        j.total = total;
        j.etag = etag.clone();
        j.last_modified = modified.clone();
        j.resume_supported = etag.is_some() || modified.is_some();
    })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial)
        .await
        .context("Cannot write partial download")?;
    let mut stream = response.bytes_stream();
    let started = Instant::now();
    let mut emitted = Instant::now();
    let mut downloaded = offset;
    loop {
        let next = tokio::select! { _=cancel.cancelled()=>{ file.flush().await?; update(&store,&job.id,|j|j.downloaded=downloaded)?; anyhow::bail!("Paused"); }, next=stream.next()=>next };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.context("HTTP transfer interrupted; partial file kept")?;
        file.write_all(&chunk)
            .await
            .context("Could not write download; partial file kept")?;
        downloaded += chunk.len() as u64;
        // Persist the first bytes immediately: a quick pause still requires
        // restart confirmation when this server cannot resume.
        if downloaded - offset == chunk.len() as u64
            || emitted.elapsed() > Duration::from_millis(200)
        {
            let speed = (downloaded - offset) as f64 / started.elapsed().as_secs_f64().max(0.001);
            update(&store, &job.id, |j| {
                j.downloaded = downloaded;
                j.speed = speed;
                j.eta =
                    total.map(|n| (n.saturating_sub(downloaded) as f64 / speed.max(1.0)) as u64);
            })?;
            emitted = Instant::now();
        }
    }
    if let Some(total) = total {
        anyhow::ensure!(
            downloaded == total,
            "Download ended early; partial file kept"
        );
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    anyhow::ensure!(!cancel.is_cancelled(), "Paused");
    // Atomic no-clobber publication: hard_link fails if a destination appeared mid-transfer.
    tokio::fs::hard_link(&partial, &final_path)
        .await
        .context("Cannot publish download without overwriting an existing file")?;
    tokio::fs::remove_file(&partial).await?;
    update(&store, &job.id, |j| {
        j.downloaded = downloaded;
        j.total = Some(downloaded);
    })?;
    Ok(vec![final_path])
}

fn validate_resume_response(response: &reqwest::Response, job: &Job, offset: u64) -> Result<u64> {
    let range = response
        .headers()
        .get("content-range")
        .and_then(|value| value.to_str().ok())
        .context("Missing resume range")?;
    let (start, end, total) = parse_content_range(range)?;
    anyhow::ensure!(
        start == offset && end.checked_add(1) == Some(total),
        "Server returned an incomplete or mismatched resume range; restart requires confirmation"
    );
    if let Some(length) = response.content_length() {
        anyhow::ensure!(
            length == total - start,
            "Resume body length does not match its range"
        );
    }
    if let Some(previous) = job.total {
        anyhow::ensure!(
            previous == total,
            "Server changed the file size during resume"
        );
    }
    for (name, previous) in [
        ("etag", job.etag.as_ref()),
        ("last-modified", job.last_modified.as_ref()),
    ] {
        if let (Some(previous), Some(current)) = (previous, response.headers().get(name)) {
            anyhow::ensure!(
                current.to_str()? == previous,
                "Server changed the file during resume"
            );
        }
    }
    Ok(total)
}

fn parse_content_range(value: &str) -> Result<(u64, u64, u64)> {
    let range = value
        .strip_prefix("bytes ")
        .context("Invalid resume range unit")?;
    let (span, total) = range.split_once('/').context("Invalid resume range")?;
    let (start, end) = span.split_once('-').context("Invalid resume range")?;
    let (start, end, total): (u64, u64, u64) = (start.parse()?, end.parse()?, total.parse()?);
    anyhow::ensure!(start <= end && end < total, "Invalid resume range bounds");
    Ok((start, end, total))
}

async fn ytdlp(
    job: Job,
    paths: &Paths,
    settings: &Settings,
    store: SharedStore,
    cancel: CancellationToken,
) -> Result<Vec<PathBuf>> {
    let mut cmd = ytdlp_base(&job.spec, paths, settings)?;
    cmd.args([
        "--newline",
        "--progress",
        "--no-overwrites",
        "--continue",
        "--windows-filenames",
        "--trim-filenames",
        "160",
        "--progress-template",
        "download:OMNI_PROGRESS:%(progress)j",
        "--print",
        "before_dl:OMNI_TITLE:%(title)j",
        "--print",
        "after_move:OMNI_FILE:%(filepath)j",
    ]);
    cmd.arg("-P").arg(&job.destination).args(["-o","%(playlist_title,channel|Media).80B/%(playlist_index|0)03d - %(title).120B [%(id)s].%(ext)s"]);
    if job.spec.options.audio_only {
        require(settings, "ffmpeg")?;
        cmd.arg("-x")
            .arg("--audio-format")
            .arg(job.spec.options.format.as_deref().unwrap_or("mp3"));
    } else {
        if let Some(q) = job.spec.options.quality {
            cmd.arg("-f")
                .arg(format!("bv*[height<={q}]+ba/b[height<={q}]/b"));
        }
        if let Some(ref format) = job.spec.options.format {
            require(settings, "ffmpeg")?;
            cmd.arg("--merge-output-format").arg(format);
        }
    }
    if let Some(ref subs) = job.spec.options.subtitles {
        cmd.args(["--write-subs", "--sub-langs"]).arg(subs);
    }
    if let Some(ref items) = job.spec.options.playlist_items {
        cmd.arg("--playlist-items").arg(items);
    }
    cmd.arg("--").arg(&job.spec.source);
    let mut files = vec![];
    run_lines(cmd, cancel, |line| {
        if let Some(json) = line.strip_prefix("OMNI_PROGRESS:") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
                update(&store, &job.id, |j| {
                    j.downloaded = v["downloaded_bytes"].as_u64().unwrap_or(0);
                    j.total = v["total_bytes"]
                        .as_u64()
                        .or(v["total_bytes_estimate"].as_u64());
                    j.speed = v["speed"].as_f64().filter(|n| n.is_finite()).unwrap_or(0.0);
                    j.eta = v["eta"].as_u64();
                })?;
            }
        } else if let Some(json) = line.strip_prefix("OMNI_TITLE:") {
            if let Ok(title) = serde_json::from_str::<String>(json) {
                update(&store, &job.id, |j| j.title = title)?;
            }
        } else if let Some(json) = line.strip_prefix("OMNI_FILE:") {
            if let Ok(path) = serde_json::from_str::<String>(json) {
                files.push(PathBuf::from(path));
            }
        }
        Ok(())
    })
    .await?;
    anyhow::ensure!(
        !files.is_empty(),
        "Extractor finished without reporting a saved file"
    );
    for path in &files {
        anyhow::ensure!(path.is_file(), "Extractor reported a missing output file");
    }
    Ok(files)
}

async fn convert(
    job: Job,
    settings: &Settings,
    store: SharedStore,
    cancel: CancellationToken,
) -> Result<Vec<PathBuf>> {
    let format = job.spec.options.format.as_deref().unwrap_or("mp4");
    let final_path = job
        .destination
        .join(format!("{}.{}", safe_filename(&job.title), format));
    let temp = job
        .destination
        .join(format!("{}.working.{}", safe_filename(&job.title), format));
    anyhow::ensure!(!final_path.exists(), "Converted file already exists");
    let mut cmd = Command::new(require(settings, "ffmpeg")?);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
        .arg(&job.spec.source);
    match format {
        "mp3" => {
            cmd.args(["-vn", "-c:a", "libmp3lame", "-q:a", "2"]);
        }
        "flac" => {
            cmd.args(["-vn", "-c:a", "flac"]);
        }
        "wav" => {
            cmd.args(["-vn", "-c:a", "pcm_s16le"]);
        }
        "m4a" => {
            cmd.args(["-vn", "-c:a", "aac", "-b:a", "192k"]);
        }
        "opus" => {
            cmd.args(["-vn", "-c:a", "libopus", "-b:a", "128k"]);
        }
        "webm" => {
            cmd.args(["-c:v", "libvpx-vp9", "-c:a", "libopus"]);
        }
        "png" | "jpg" => {
            cmd.args(["-frames:v", "1"]);
        }
        _ => {
            cmd.args([
                "-c:v", "libx264", "-preset", "medium", "-crf", "23", "-c:a", "aac",
            ]);
        }
    }
    cmd.args(["-progress", "pipe:1", "-nostats"]).arg(&temp);
    run_lines(cmd, cancel, |line| {
        if let Some(n) = line
            .strip_prefix("total_size=")
            .and_then(|s| s.parse().ok())
        {
            update(&store, &job.id, |j| j.downloaded = n)?;
        }
        Ok(())
    })
    .await?;
    tokio::fs::hard_link(&temp, &final_path).await?;
    tokio::fs::remove_file(&temp).await?;
    let size = tokio::fs::metadata(&final_path).await?.len();
    update(&store, &job.id, |j| {
        j.downloaded = size;
        j.total = Some(size);
    })?;
    Ok(vec![final_path])
}
async fn torrent(
    job: Job,
    settings: &Settings,
    store: SharedStore,
    cancel: CancellationToken,
) -> Result<Vec<PathBuf>> {
    let mut cmd = Command::new(require(settings, "aria2c")?);
    cmd.args([
        "--seed-time=0",
        "--enable-rpc=false",
        "--enable-color=false",
        "--summary-interval=1",
        "--continue=true",
        "--allow-overwrite=false",
        "--auto-file-renaming=true",
        "--dir",
    ])
    .arg(&job.destination)
    .arg("--")
    .arg(&job.spec.source);
    run_lines(cmd, cancel, |_line| {
        let size = collect_files(&job.destination)?
            .iter()
            .filter_map(|p| p.metadata().ok())
            .map(|m| m.len())
            .sum();
        update(&store, &job.id, |j| j.downloaded = size)
    })
    .await?;
    let files = collect_files(&job.destination)?;
    anyhow::ensure!(!files.is_empty(), "Torrent completed without saved files");
    Ok(files)
}
fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = vec![];
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(collect_files(&path)?);
        } else if path.extension().and_then(|s| s.to_str()) != Some("aria2") {
            files.push(path);
        }
    }
    Ok(files)
}

pub fn prepare(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(target_os = "linux")]
                {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                }
                Ok(())
            });
        }
    }
}

pub struct ProcessTree {
    #[cfg(windows)]
    handle: isize,
    #[cfg(unix)]
    pid: i32,
    #[cfg(unix)]
    guardian: std::process::Child,
}
impl ProcessTree {
    pub fn attach(child: &tokio::process::Child) -> Result<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::*;
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                anyhow::ensure!(!handle.is_null(), "Cannot create child process group");
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                ) == 0
                    || AssignProcessToJobObject(
                        handle,
                        child.raw_handle().context("Missing process handle")?,
                    ) == 0
                {
                    windows_sys::Win32::Foundation::CloseHandle(handle);
                    anyhow::bail!("Cannot attach child process cleanup group");
                }
                Ok(Self {
                    handle: handle as isize,
                })
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let pid = child.id().context("Missing child PID")? as i32;
            // The guardian outlives a crashed worker. EOF on its private stdin
            // triggers cleanup of the entire dependency group on Linux/macOS.
            let guardian = std::process::Command::new(std::env::current_exe()?)
                .args(["__watch-group", &pid.to_string()])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .context("Cannot start dependency cleanup guardian")?;
            Ok(Self { pid, guardian })
        }
    }
}
impl Drop for ProcessTree {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle as _);
        }
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
            drop(self.guardian.stdin.take());
            let _ = self.guardian.wait();
        }
    }
}
pub async fn capture(mut cmd: Command, timeout: Duration) -> Result<Vec<u8>> {
    prepare(&mut cmd);
    let mut child = cmd.spawn().context("Cannot start dependency")?;
    let tree = ProcessTree::attach(&child)?;
    let stdout = child.stdout.take().context("Missing stdout")?;
    let stderr = child.stderr.take().context("Missing stderr")?;
    let out = tokio::spawn(async move {
        let mut bytes = vec![];
        stdout
            .take(MAX_FRAME as u64)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let err = tokio::spawn(async move {
        let mut bytes = vec![];
        stderr
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let result = tokio::time::timeout(timeout, child.wait()).await;
    if result.is_err() {
        let _ = child.kill().await;
        drop(tree);
        out.abort();
        err.abort();
        anyhow::bail!("Metadata inspection timed out");
    }
    let status = result??;
    let output = out.await??;
    let error = err.await??;
    drop(tree);
    anyhow::ensure!(
        status.success(),
        "{}",
        redact(&String::from_utf8_lossy(&error))
    );
    anyhow::ensure!(
        output.len() < MAX_FRAME,
        "Metadata too large; inspect a smaller collection"
    );
    Ok(output)
}
pub async fn run_lines(
    mut cmd: Command,
    cancel: CancellationToken,
    mut callback: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    prepare(&mut cmd);
    let mut child = cmd.spawn().context("Cannot start dependency")?;
    let tree = ProcessTree::attach(&child)?;
    let mut stdout = BufReader::new(child.stdout.take().context("Missing stdout")?);
    let stderr = child.stderr.take().context("Missing stderr")?;
    let err_task = tokio::spawn(async move {
        let mut stderr = BufReader::new(stderr);
        let mut tail = std::collections::VecDeque::new();
        loop {
            let mut chunk = Vec::new();
            match (&mut stderr)
                .take(64 * 1024)
                .read_until(b'\n', &mut chunk)
                .await
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if tail.len() >= 12 {
                tail.pop_front();
            }
            tail.push_back(
                String::from_utf8_lossy(&chunk)
                    .trim()
                    .chars()
                    .take(2000)
                    .collect::<String>(),
            );
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });
    let result:Result<()>=async {
        loop {
            let mut line=String::new();
            let mut bounded=(&mut stdout).take(1024*1024);
            let read=tokio::select!{ _=cancel.cancelled()=>anyhow::bail!("Paused"), n=bounded.read_line(&mut line)=>n? };
            if read==0{break;} anyhow::ensure!(read<1024*1024,"Dependency emitted an oversized record"); callback(line.trim())?;
        }
        let status=tokio::select!{_=cancel.cancelled()=>anyhow::bail!("Paused"),r=child.wait()=>r?};
        if !status.success(){anyhow::bail!("Dependency exited with {status}");} Ok(())
    }.await;
    if result.is_err() {
        let _ = child.kill().await;
    }
    drop(tree);
    let tail = tokio::time::timeout(Duration::from_secs(2), err_task)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    result.map_err(|error| {
        if tail.is_empty() {
            error
        } else {
            anyhow::anyhow!("{}: {}", error, redact(&tail))
        }
    })
}
