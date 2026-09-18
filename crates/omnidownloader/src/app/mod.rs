pub mod tui;
use crate::{
    engine::{self, backends, client::Client, paths::Paths},
    protocol::*,
};
use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "omnidownloader",
    version,
    about = "Your downloads. One beautiful terminal."
)]
struct Cli {
    #[arg(long, global = true, help = "Output newline-delimited JSON")]
    json: bool,
    #[arg(
        long,
        global = true,
        help = "Separate application data/profile directory"
    )]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}
#[derive(Subcommand)]
enum Commands {
    /// Internal crash-cleanup guardian; stdin is its lifetime pipe.
    #[cfg(unix)]
    #[command(name = "__watch-group", hide = true)]
    WatchGroup { pid: i32 },
    /// Open the interactive dashboard
    Tui,
    /// Download video, audio, a playlist, or a direct file
    Download {
        url: String,
        #[command(flatten)]
        options: Options,
    },
    /// Inspect media without downloading
    Info {
        url: String,
        #[command(flatten)]
        options: Options,
    },
    /// Download one URL per line from a UTF-8 text file
    Batch {
        file: PathBuf,
        #[command(flatten)]
        options: Options,
    },
    /// Manage the shared persistent queue
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    /// Manage the background worker
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Inspect dependencies and setup guidance
    Doctor,
    /// Import cookies and manage accounts
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Convert local audio/video or extract a still image
    Convert {
        file: PathBuf,
        #[arg(long, default_value = "mp4")]
        format: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        detach: bool,
    },
    /// Find downloaded music, books, and other files
    Library {
        #[arg(short, long)]
        search: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// Experimental course routing; see courses providers for availability
    Courses {
        #[command(subcommand)]
        command: CourseCommand,
    },
    /// Configure optional subprocess extensions
    Plugins {
        #[command(subcommand)]
        command: PluginCommand,
    },
    /// View or change application settings
    Settings {
        #[command(subcommand)]
        command: Option<SettingsCommand>,
    },
    /// Local file utilities
    Tools {
        #[command(subcommand)]
        command: ToolCommand,
    },
    /// Download a torrent or magnet link using optional aria2
    Torrent {
        source: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        detach: bool,
    },
    #[command(hide = true)]
    Render {
        file: PathBuf,
        #[arg(long, default_value = "120")]
        width: u16,
        #[arg(long, default_value = "35")]
        height: u16,
    },
}
#[derive(Args, Clone, Default)]
struct Options {
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long)]
    quality: Option<u32>,
    #[arg(long)]
    audio_only: bool,
    #[arg(long)]
    subtitles: Option<String>,
    #[arg(long)]
    format: Option<String>,
    #[arg(long)]
    proxy: Option<String>,
    #[arg(long)]
    account: Option<String>,
    #[arg(long)]
    playlist_items: Option<String>,
    #[arg(long)]
    direct: bool,
    #[arg(long)]
    filename: Option<String>,
    #[arg(long)]
    detach: bool,
}
impl Options {
    fn spec(&self, source: String) -> Result<JobSpec> {
        Ok(JobSpec {
            source,
            kind: JobKind::Download,
            options: DownloadOptions {
                output: self.output.as_ref().map(|p| absolute(p)).transpose()?,
                quality: self.quality,
                audio_only: self.audio_only,
                subtitles: self.subtitles.clone(),
                format: self.format.clone(),
                proxy: self.proxy.clone(),
                account: self.account.clone(),
                playlist_items: self.playlist_items.clone(),
                direct: self.direct,
                filename: self.filename.clone(),
            },
        })
    }
}
#[derive(Subcommand)]
enum QueueCommand {
    List,
    Pause {
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    Resume {
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        allow_restart: bool,
    },
    Cancel {
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    Retry {
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        allow_restart: bool,
    },
}
#[derive(Subcommand)]
enum WorkerCommand {
    Status,
    Stop,
    #[command(hide = true)]
    Run,
}
#[derive(Subcommand)]
enum AuthCommand {
    ImportCookies {
        file: PathBuf,
        #[arg(long, default_value = "default")]
        name: String,
    },
    List,
    Remove {
        name: String,
    },
    Login {
        provider: String,
    },
}
#[derive(Subcommand)]
enum CourseCommand {
    Providers,
    Inspect {
        url: String,
        #[command(flatten)]
        options: Options,
    },
    Download {
        url: String,
        #[command(flatten)]
        options: Options,
    },
}
#[derive(Subcommand)]
enum PluginCommand {
    List,
    Add {
        manifest: PathBuf,
    },
    Inspect {
        id: String,
        url: String,
    },
    Run {
        id: String,
        url: String,
        #[command(flatten)]
        options: Options,
    },
}
#[derive(Subcommand)]
enum SettingsCommand {
    Output { directory: PathBuf },
    Concurrency { count: usize },
    Tool { name: String, path: PathBuf },
}
#[derive(Subcommand)]
enum ToolCommand {
    Hash { file: PathBuf },
    Metadata { file: PathBuf },
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}
pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    #[cfg(unix)]
    if let Some(Commands::WatchGroup { pid }) = &cli.command {
        anyhow::ensure!(*pid > 1, "Invalid dependency process group");
        use std::io::Read;
        let mut byte = [0_u8; 1];
        loop {
            match std::io::stdin().read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        unsafe {
            libc::kill(-*pid, libc::SIGKILL);
        }
        return Ok(());
    }
    if cli.command.is_none()
        && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() || cli.json)
    {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    }
    let paths = Paths::new(cli.data_dir)?;
    if matches!(
        cli.command,
        Some(Commands::Worker {
            command: WorkerCommand::Run
        })
    ) {
        return engine::worker::run(paths).await;
    }
    if let Some(Commands::Render {
        file,
        width,
        height,
    }) = &cli.command
    {
        return tui::render_preview(file, *width, *height);
    }
    if let Some(Commands::Worker { command }) = &cli.command {
        let mut client = match Client::connect(&paths).await {
            Ok(c) => c,
            Err(_) => {
                emit(cli.json, "worker", &serde_json::json!({"running":false}))?;
                return Ok(());
            }
        };
        return match command {
            WorkerCommand::Status => {
                let s: Snapshot = client.call(Request::Snapshot).await?;
                emit(
                    cli.json,
                    "worker",
                    &serde_json::json!({"running":true,"pid":s.worker_pid,"active":s.jobs.iter().filter(|j|j.status==Status::Active).count()}),
                )
            }
            WorkerCommand::Stop => {
                let value: serde_json::Value = client.call(Request::Stop).await?;
                emit(cli.json, "worker", &value)
            }
            _ => unreachable!(),
        };
    }
    let mut client = Client::ensure(&paths).await?;
    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => {
            anyhow::ensure!(
                std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
                "Open the dashboard in an interactive terminal"
            );
            tui::run(paths, client).await?;
        }
        Commands::Download { url, options } => {
            submit_wait(
                &mut client,
                vec![options.spec(url)?],
                options.detach,
                cli.json,
            )
            .await?
        }
        Commands::Info { url, options } => {
            let info: MediaInfo = client.call(Request::Inspect(options.spec(url)?)).await?;
            emit(cli.json, "media", &info)?;
        }
        Commands::Batch { file, options } => {
            let text = std::fs::read_to_string(file)?;
            let specs = text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('#'))
                .map(|s| options.spec(s.to_string()))
                .collect::<Result<Vec<_>>>()?;
            submit_wait(&mut client, specs, options.detach, cli.json).await?;
        }
        Commands::Queue { command } => {
            let snapshot: Snapshot = client.call(Request::Snapshot).await?;
            if matches!(command, QueueCommand::List) {
                if cli.json {
                    emit(true, "queue", &public_jobs(snapshot.jobs))?;
                } else {
                    print_jobs(&snapshot.jobs);
                }
                return Ok(());
            }
            let (ids, all, action, allow_restart) = match command {
                QueueCommand::Pause { ids, all } => (ids, all, Control::Pause, false),
                QueueCommand::Resume {
                    ids,
                    all,
                    allow_restart,
                } => (ids, all, Control::Resume, allow_restart),
                QueueCommand::Cancel { ids, all } => (ids, all, Control::Cancel, false),
                QueueCommand::Retry {
                    ids,
                    all,
                    allow_restart,
                } => (ids, all, Control::Retry, allow_restart),
                _ => unreachable!(),
            };
            let ids = resolve_ids(&snapshot.jobs, &ids, all, action)?;
            let result: serde_json::Value = client
                .call(Request::Control {
                    ids,
                    action,
                    allow_restart,
                })
                .await?;
            emit(cli.json, "queue", &result)?;
        }
        Commands::Doctor => {
            let deps: Vec<Dependency> = client.call(Request::Doctor).await?;
            if cli.json {
                emit(true, "dependencies", &deps)?;
            } else {
                for dep in deps {
                    if let Some(path) = dep.path {
                        println!("[OK]      {:10} {}", dep.name, path.display());
                    } else {
                        println!(
                            "[Missing] {:10} {}\n          {}",
                            dep.name, dep.purpose, dep.guidance
                        );
                    }
                }
            }
        }
        Commands::Auth { command } => match command {
            AuthCommand::ImportCookies { file, name } => {
                let account: Account = client
                    .call(Request::ImportCookies {
                        file: absolute(&file)?,
                        name,
                    })
                    .await?;
                emit(cli.json, "account", &account)?;
            }
            AuthCommand::List => {
                let s: Snapshot = client.call(Request::Snapshot).await?;
                emit(cli.json, "accounts", &s.accounts)?;
            }
            AuthCommand::Remove { name } => {
                let v: serde_json::Value = client.call(Request::RemoveAccount(name)).await?;
                emit(cli.json, "account", &v)?;
            }
            AuthCommand::Login { provider } => {
                open_login(&provider)?;
                emit(
                    cli.json,
                    "authentication",
                    &serde_json::json!({"authenticated":false,"message":"Browser opened for login. Export Netscape cookies and import them with auth import-cookies; opening the browser does not connect an account."}),
                )?;
            }
        },
        Commands::Convert {
            file,
            format,
            output,
            detach,
        } => {
            let spec = JobSpec {
                source: absolute(&file)?.to_string_lossy().into(),
                kind: JobKind::Convert,
                options: DownloadOptions {
                    format: Some(format),
                    output: output.map(|p| absolute(&p)).transpose()?,
                    ..Default::default()
                },
            };
            submit_wait(&mut client, vec![spec], detach, cli.json).await?;
        }
        Commands::Library {
            search,
            kind,
            export,
        } => {
            let snapshot: Snapshot = client.call(Request::Snapshot).await?;
            let files = library(&snapshot, search.as_deref().unwrap_or(""), kind.as_deref());
            if let Some(file) = export {
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(file)?;
                serde_json::to_writer_pretty(&mut output, &files)?;
            }
            emit(cli.json, "library", &files)?;
        }
        Commands::Courses { command } => match command {
            CourseCommand::Providers => emit(
                cli.json,
                "providers",
                &serde_json::json!([
                    {"id":"udemy","status":"experimental yt-dlp routing; live account verification pending","dedicated_adapter":false},
                    {"id":"hotmart","status":"planned; no dedicated adapter implemented","dedicated_adapter":false},
                    {"id":"telegram","status":"public embeds may work through yt-dlp; account/library adapter planned","dedicated_adapter":false}
                ]),
            )?,
            CourseCommand::Inspect { url, options } => {
                validate_course(&url)?;
                let info: MediaInfo = client.call(Request::Inspect(options.spec(url)?)).await?;
                emit(cli.json, "course", &info)?;
            }
            CourseCommand::Download { url, options } => {
                validate_course(&url)?;
                submit_wait(
                    &mut client,
                    vec![options.spec(url)?],
                    options.detach,
                    cli.json,
                )
                .await?;
            }
        },
        Commands::Plugins { command } => match command {
            PluginCommand::List => {
                let v: Vec<PluginManifest> = client.call(Request::Plugins).await?;
                emit(cli.json, "plugins", &v)?;
            }
            PluginCommand::Add { manifest } => {
                let v: PluginManifest = client
                    .call(Request::RegisterPlugin(absolute(&manifest)?))
                    .await?;
                emit(cli.json, "plugin", &v)?;
            }
            PluginCommand::Inspect { id, url } => {
                let spec = JobSpec {
                    source: url,
                    kind: JobKind::Plugin(id),
                    options: DownloadOptions::default(),
                };
                let info: MediaInfo = client.call(Request::Inspect(spec)).await?;
                emit(cli.json, "media", &info)?;
            }
            PluginCommand::Run { id, url, options } => {
                let mut spec = options.spec(url)?;
                spec.kind = JobKind::Plugin(id);
                submit_wait(&mut client, vec![spec], options.detach, cli.json).await?;
            }
        },
        Commands::Settings { command } => {
            let mut s: Snapshot = client.call(Request::Snapshot).await?;
            if let Some(command) = command {
                match command {
                    SettingsCommand::Output { directory } => {
                        s.settings.output = absolute(&directory)?
                    }
                    SettingsCommand::Concurrency { count } => s.settings.concurrency = count,
                    SettingsCommand::Tool { name, path } => {
                        anyhow::ensure!(
                            ["yt-dlp", "ffmpeg", "ffprobe", "aria2c", "deno"]
                                .contains(&name.as_str()),
                            "Unknown tool"
                        );
                        s.settings.tools.insert(name, std::fs::canonicalize(path)?);
                    }
                }
                let _: Settings = client.call(Request::Settings(s.settings.clone())).await?;
            }
            emit(cli.json, "settings", &s.settings)?;
        }
        Commands::Tools { command } => match command {
            ToolCommand::Hash { file } => {
                use sha2::{Digest, Sha256};
                use std::io::Read;
                let mut reader = std::fs::File::open(&file)?;
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 65536];
                loop {
                    let n = reader.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    hash.update(&buffer[..n]);
                }
                emit(
                    cli.json,
                    "hash",
                    &serde_json::json!({"file":file,"sha256":format!("{:x}",hash.finalize())}),
                )?;
            }
            ToolCommand::Metadata { file } => {
                let s: Snapshot = client.call(Request::Snapshot).await?;
                let info = backends::inspect(
                    &JobSpec {
                        source: absolute(&file)?.to_string_lossy().into(),
                        kind: JobKind::Convert,
                        options: DownloadOptions::default(),
                    },
                    &paths,
                    &s.settings,
                )
                .await?;
                emit(cli.json, "metadata", &info)?;
            }
        },
        Commands::Torrent {
            source,
            output,
            detach,
        } => {
            let source = if source.starts_with("magnet:") || source.starts_with("http") {
                source
            } else {
                absolute(Path::new(&source))?.to_string_lossy().into()
            };
            submit_wait(
                &mut client,
                vec![JobSpec {
                    source,
                    kind: JobKind::Torrent,
                    options: DownloadOptions {
                        output: output.map(|p| absolute(&p)).transpose()?,
                        ..Default::default()
                    },
                }],
                detach,
                cli.json,
            )
            .await?;
        }
        Commands::Worker { .. } | Commands::Render { .. } => unreachable!(),
        #[cfg(unix)]
        Commands::WatchGroup { .. } => unreachable!(),
    }
    Ok(())
}
fn validate_course(source: &str) -> Result<()> {
    let url = url::Url::parse(source)?;
    let host = url.host_str().unwrap_or("");
    anyhow::ensure!(
        host != "hotmart.com" && !host.ends_with(".hotmart.com"),
        "Hotmart course support is planned; no dedicated adapter is implemented yet"
    );
    anyhow::ensure!(
        ["udemy.com", "hotmart.com"]
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}"))),
        "Courses accepts Udemy and Hotmart URLs; use download for other sources"
    );
    Ok(())
}
pub fn open_login(provider: &str) -> Result<()> {
    let url = match provider.to_ascii_lowercase().as_str() {
        "udemy" => "https://www.udemy.com/join/login-popup/",
        "hotmart" => "https://sso.hotmart.com/login",
        "youtube" => "https://accounts.google.com/",
        _ => anyhow::bail!("Browser login supports udemy, hotmart, or youtube"),
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url)
        .spawn()
        .context("Could not open browser for login")?;
    Ok(())
}
fn emit(json: bool, kind: &str, value: &impl Serialize) -> Result<()> {
    if json {
        println!("{}", serde_json::json!({"type":kind,"data":value}));
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}
fn public_jobs(mut jobs: Vec<Job>) -> Vec<Job> {
    for j in &mut jobs {
        j.spec.source = engine::redact(&j.spec.source);
        j.spec.options.proxy = j.spec.options.proxy.as_ref().map(|s| engine::redact(s));
    }
    jobs
}
fn print_jobs(jobs: &[Job]) {
    println!("{:<10} {:<12} {:>8}  TITLE", "ID", "STATUS", "SIZE");
    for job in jobs {
        println!(
            "{:<10} {:<12} {:>8}  {}",
            &job.id[..8],
            job.status.label(),
            tui::bytes(job.downloaded),
            job.title
        );
    }
}
fn resolve_ids(jobs: &[Job], ids: &[String], all: bool, action: Control) -> Result<Vec<String>> {
    if all {
        return Ok(jobs
            .iter()
            .filter(|j| match action {
                Control::Pause => j.status.pending(),
                Control::Resume => j.status == Status::Paused,
                Control::Retry => j.status == Status::Failed,
                Control::Cancel => !j.status.terminal(),
            })
            .map(|j| j.id.clone())
            .collect());
    }
    anyhow::ensure!(!ids.is_empty(), "Provide job IDs or --all");
    ids.iter()
        .map(|id| {
            let matching = jobs
                .iter()
                .filter(|j| j.id.starts_with(id))
                .collect::<Vec<_>>();
            anyhow::ensure!(
                matching.len() == 1,
                "Job prefix {id} is unknown or ambiguous"
            );
            Ok(matching[0].id.clone())
        })
        .collect()
}
async fn submit_wait(
    client: &mut Client,
    specs: Vec<JobSpec>,
    detach: bool,
    json: bool,
) -> Result<()> {
    let ids: Vec<String> = client.call(Request::Submit(specs)).await?;
    emit(json, "submitted", &ids)?;
    if detach {
        return Ok(());
    }
    loop {
        let snapshot: Snapshot = client.call(Request::Snapshot).await?;
        let jobs: Vec<Job> = snapshot
            .jobs
            .into_iter()
            .filter(|j| ids.contains(&j.id))
            .collect();
        if json {
            emit(true, "progress", &public_jobs(jobs.clone()))?;
        } else if std::io::stderr().is_terminal() {
            eprint!(
                "\r{} jobs · {} done · {} active   ",
                jobs.len(),
                jobs.iter()
                    .filter(|j| j.status == Status::Completed)
                    .count(),
                jobs.iter().filter(|j| j.status == Status::Active).count()
            );
        }
        if jobs.len() == ids.len()
            && jobs
                .iter()
                .all(|j| j.status.terminal() || j.status == Status::Paused)
        {
            if !json {
                eprintln!();
                print_jobs(&jobs);
                for job in &jobs {
                    for file in &job.files {
                        println!("Saved: {}", file.display());
                    }
                }
            }
            anyhow::ensure!(
                jobs.iter().all(|j| j.status == Status::Completed),
                "Some jobs did not complete; inspect queue list for details"
            );
            if json {
                emit(true, "complete", &public_jobs(jobs))?;
            }
            return Ok(());
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(500))=>{},_=tokio::signal::ctrl_c()=>{if !json{eprintln!("\nDetached. Downloads continue; use queue pause --all to pause them.");}else{emit(true,"detached",&ids)?;}return Ok(());}}
    }
}
pub fn library(snapshot: &Snapshot, search: &str, kind: Option<&str>) -> Vec<serde_json::Value> {
    let needle = search.to_lowercase();
    snapshot.jobs.iter().filter(|j|j.status==Status::Completed).flat_map(|j|j.files.iter().map(move|f|(j,f))).filter(|(j,p)|{
        let ext=p.extension().and_then(|x|x.to_str()).unwrap_or("").to_lowercase();
        let group=if ["mp3","flac","wav","m4a","opus"].contains(&ext.as_str()){"music"}else if ["pdf","epub","mobi"].contains(&ext.as_str()){"books"}else{"other"};
        (needle.is_empty()||j.title.to_lowercase().contains(&needle)||p.to_string_lossy().to_lowercase().contains(&needle))&&kind.is_none_or(|k|k==group)
    }).map(|(job,path)|serde_json::json!({"title":job.title,"file":path,"exists":path.is_file(),"job_id":job.id})).collect()
}
