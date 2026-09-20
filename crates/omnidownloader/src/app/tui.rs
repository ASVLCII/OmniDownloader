use crate::{
    engine::{client::Client, paths::Paths},
    protocol::*,
};
use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
};
use std::{collections::BTreeSet, io, path::Path, time::Duration};
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Downloads,
    Library,
    Tools,
    Accounts,
}
impl Page {
    fn actions(self) -> Vec<(&'static str, Action)> {
        match self {
            Self::Library => vec![
                ("Details", Action::Details),
                ("Export library", Action::Export),
            ],
            Self::Tools => vec![
                ("Check tools", Action::Doctor),
                ("Add plugin", Action::Plugin),
            ],
            Self::Accounts => vec![
                ("Import cookies", Action::Import),
                ("Udemy login", Action::Login("udemy".into())),
                ("Hotmart login", Action::Login("hotmart".into())),
            ],
            Self::Downloads => vec![
                ("Pause", Action::Pause),
                ("Resume", Action::Resume),
                ("Cancel", Action::Cancel),
                ("Retry", Action::Retry),
                ("Details", Action::Details),
            ],
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Downloads => "Downloads",
            Self::Library => "Library",
            Self::Tools => "Tools",
            Self::Accounts => "Accounts",
        }
    }
}

#[derive(Clone, Debug)]
enum Action {
    Page(Page),
    Tab(usize),
    Row(usize),
    Mark(usize),
    Add,
    Convert,
    Import,
    Settings,
    Inspect,
    Submit,
    Close,
    Quit,
    Exit,
    Keep,
    Pause,
    Resume,
    Cancel,
    Retry,
    Details,
    Search,
    Field(usize),
    Toggle(usize),
    Playlist(usize),
    PlaylistAll,
    ApplyPlaylist,
    Doctor,
    Plugin,
    Login(String),
    Export,
    Help,
    Compose,
    ChooseCommand(usize),
    Fill(String),
    QueueJob(String),
    Issues,
    OpenFolder,
    CopyPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlashCommand {
    Add,
    Queue,
    Library,
    Convert,
    Account,
    System,
    Help,
    Quit,
}
const COMMANDS: [(SlashCommand, &str, &str); 8] = [
    (SlashCommand::Add, "add", "Download a URL or import a list"),
    (SlashCommand::Queue, "queue", "View and control downloads"),
    (SlashCommand::Library, "library", "Browse completed files"),
    (
        SlashCommand::Convert,
        "convert",
        "Convert a local media file",
    ),
    (SlashCommand::Account, "account", "Manage cookie accounts"),
    (
        SlashCommand::System,
        "system",
        "Tools, settings, plugins and worker",
    ),
    (
        SlashCommand::Help,
        "help",
        "Commands and keyboard shortcuts",
    ),
    (SlashCommand::Quit, "quit", "Exit safely"),
];

fn parse_command(input: &str) -> Result<(SlashCommand, &str)> {
    let input = input.trim();
    let command = input.strip_prefix('/').context("Commands start with /")?;
    let (name, rest) = command
        .split_once(char::is_whitespace)
        .unwrap_or((command, ""));
    let command = COMMANDS
        .iter()
        .find(|(_, candidate, _)| *candidate == name)
        .map(|(command, _, _)| *command)
        .with_context(|| {
            format!("Unknown command /{name}. Type /help to see available commands.")
        })?;
    Ok((command, rest.trim()))
}

fn command_match(query: &str, value: &str, summary: &str) -> bool {
    let query = query.to_lowercase();
    if query.is_empty() {
        return true;
    }
    let candidate = format!("{value} {summary}").to_lowercase();
    candidate.contains(&query)
        || query
            .chars()
            .try_fold(candidate.chars(), |mut chars, wanted| {
                chars.find(|candidate| *candidate == wanted).map(|_| chars)
            })
            .is_some()
}

// ponytail: global palette reuses the suggestion popup; no second palette system.
fn palette_query(ui: &Ui) -> String {
    ui.composer.trim().trim_start_matches('/').to_string()
}
#[derive(Clone)]
struct Hit {
    area: Rect,
    action: Action,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum FormKind {
    Add,
    Convert,
    Import,
    Settings,
    Plugin,
    Export,
}
#[derive(Clone)]
struct Field {
    label: String,
    value: String,
    toggle: bool,
}
#[derive(Clone)]
struct Form {
    kind: FormKind,
    fields: Vec<Field>,
    focus: usize,
    action: usize,
}
#[derive(Clone)]
enum Dialog {
    Form(Form),
    Exit {
        keep: bool,
        focus: usize,
    },
    Info {
        title: String,
        text: String,
        scroll: u16,
    },
    Playlist {
        info: MediaInfo,
        selected: BTreeSet<usize>,
        cursor: usize,
        form: Form,
    },
    Restart {
        ids: Vec<String>,
        action: Control,
    },
    RemoveAccount {
        name: String,
    },
}
#[derive(Clone, Copy)]
struct Palette {
    bg: Color,
    panel: Color,
    raised: Color,
    fg: Color,
    muted: Color,
    line: Color,
    cyan: Color,
    green: Color,
    amber: Color,
    red: Color,
    select: Color,
}
impl Palette {
    fn new() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Self {
                bg: Color::Reset,
                panel: Color::Reset,
                raised: Color::Reset,
                fg: Color::Reset,
                muted: Color::Reset,
                line: Color::Reset,
                cyan: Color::Reset,
                green: Color::Reset,
                amber: Color::Reset,
                red: Color::Reset,
                select: Color::Reset,
            };
        }
        Self {
            bg: Color::Rgb(18, 17, 15),
            panel: Color::Rgb(24, 23, 20),
            raised: Color::Rgb(36, 34, 29),
            fg: Color::Rgb(226, 222, 211),
            muted: Color::Rgb(145, 140, 128),
            line: Color::Rgb(58, 55, 48),
            cyan: Color::Rgb(150, 174, 145),
            green: Color::Rgb(132, 168, 132),
            amber: Color::Rgb(204, 167, 106),
            red: Color::Rgb(207, 122, 112),
            select: Color::Rgb(45, 50, 41),
        }
    }
}
struct Ui {
    snapshot: Snapshot,
    page: Page,
    tab: usize,
    selected: usize,
    marked: BTreeSet<String>,
    table: TableState,
    focus: usize,
    detail_focus: usize,
    search: String,
    searching: bool,
    composer: String,
    composing: bool,
    global: bool,
    command_cursor: usize,
    issues_only: bool,
    dialog: Option<Dialog>,
    return_form: Option<Form>,
    hits: Vec<Hit>,
    dependencies: Vec<Dependency>,
    plugins: Vec<PluginManifest>,
    notice: String,
    busy: bool,
    connected: bool,
    tx: mpsc::UnboundedSender<(Request, String)>,
    quit: bool,
    palette: Palette,
}
impl Ui {
    fn new(snapshot: Snapshot, tx: mpsc::UnboundedSender<(Request, String)>) -> Self {
        Self {
            snapshot,
            page: Page::Downloads,
            tab: 0,
            selected: 0,
            marked: BTreeSet::new(),
            table: TableState::default(),
            focus: 2,
            detail_focus: 0,
            search: String::new(),
            searching: false,
            composer: String::new(),
            composing: false,
            global: false,
            command_cursor: 0,
            issues_only: false,
            dialog: None,
            return_form: None,
            hits: vec![],
            dependencies: vec![],
            plugins: vec![],
            notice: "Ready. Paste a link to start your first download.".into(),
            busy: false,
            connected: true,
            tx,
            quit: false,
            palette: Palette::new(),
        }
    }
    fn send(&mut self, request: Request, tag: &str) {
        self.busy = true;
        let _ = self.tx.send((request, tag.into()));
    }
    fn visible(&self) -> Vec<Job> {
        let search = self.search.to_lowercase();
        self.snapshot
            .jobs
            .iter()
            .filter(|j| {
                let page = match self.page {
                    Page::Library => j.status == Status::Completed,
                    _ => true,
                };
                let tab = if self.page == Page::Library {
                    let ext = j
                        .files
                        .first()
                        .and_then(|p| p.extension())
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    match self.tab {
                        1 => ["mp3", "flac", "m4a", "wav", "opus"].contains(&ext),
                        2 => ["pdf", "epub", "mobi"].contains(&ext),
                        _ => true,
                    }
                } else {
                    if self.issues_only {
                        return page
                            && matches!(
                                j.status,
                                Status::Paused | Status::Failed | Status::Cancelled
                            )
                            && (search.is_empty()
                                || j.title.to_lowercase().contains(&search)
                                || j.destination
                                    .to_string_lossy()
                                    .to_lowercase()
                                    .contains(&search));
                    }
                    match self.tab {
                        1 => j.status == Status::Active,
                        2 => j.status == Status::Queued,
                        3 => j.status == Status::Completed,
                        _ => !matches!(
                            j.status,
                            Status::Paused | Status::Failed | Status::Cancelled
                        ),
                    }
                };
                page && tab
                    && (search.is_empty()
                        || j.title.to_lowercase().contains(&search)
                        || j.destination
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&search))
            })
            .cloned()
            .collect()
    }
    fn ids(&self) -> Vec<String> {
        if !self.marked.is_empty() {
            self.marked.iter().cloned().collect()
        } else {
            self.visible()
                .get(self.selected)
                .map(|j| vec![j.id.clone()])
                .unwrap_or_default()
        }
    }
    fn form(&self, kind: FormKind) -> Form {
        let field = |label: &str, value: String| Field {
            label: label.into(),
            value,
            toggle: false,
        };
        let toggle = |label: &str| Field {
            label: label.into(),
            value: "false".into(),
            toggle: true,
        };
        let output = self.snapshot.settings.output.to_string_lossy().into_owned();
        let fields = match kind {
            FormKind::Add => vec![
                field("URLs · one per line", String::new()),
                field("Destination", output),
                field("Quality height · blank = best", String::new()),
                field("Format · blank = automatic", String::new()),
                field("Subtitle languages · en,pt", String::new()),
                field("Cookie account · optional", String::new()),
                field("Playlist indices · 1,3-5", String::new()),
                toggle("Audio only"),
                toggle("Direct file"),
            ],
            FormKind::Convert => vec![
                field("Input file", String::new()),
                field("Destination", output),
                field("Format · mp4, mp3, flac, wav, png", "mp4".into()),
            ],
            FormKind::Import => vec![
                field("Netscape cookies.txt file", String::new()),
                field("Account name", "default".into()),
            ],
            FormKind::Settings => {
                let mut f = vec![
                    field("Default destination", output),
                    field(
                        "Concurrent downloads · 1–16",
                        self.snapshot.settings.concurrency.to_string(),
                    ),
                ];
                for name in ["yt-dlp", "ffmpeg", "ffprobe", "aria2c", "deno"] {
                    f.push(field(
                        &format!("{name} path · blank = PATH"),
                        self.snapshot
                            .settings
                            .tools
                            .get(name)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ));
                }
                f
            }
            FormKind::Plugin => vec![field("Plugin manifest JSON file", String::new())],
            FormKind::Export => vec![field(
                "Export JSON file",
                self.snapshot
                    .settings
                    .output
                    .join("omnidownloader-library.json")
                    .to_string_lossy()
                    .into_owned(),
            )],
        };
        Form {
            kind,
            fields,
            focus: 0,
            action: 0,
        }
    }
    fn command_matches(&self) -> Vec<(String, &'static str)> {
        let input = self.composer.trim_start_matches('/');
        if let Some((command, rest)) = input.split_once(char::is_whitespace) {
            let options: &[(&str, &str)] = match command {
                "queue" => &[
                    ("active", "Show active downloads"),
                    ("queued", "Show queued downloads"),
                    ("completed", "Show completed downloads"),
                    ("issues", "Show jobs needing attention"),
                    ("pause", "Pause selected downloads"),
                    ("pause all", "Pause every active or queued download"),
                    ("resume", "Resume selected paused downloads"),
                    ("resume all", "Resume all paused downloads"),
                    ("cancel", "Cancel selected downloads"),
                    ("cancel all", "Cancel all pending downloads"),
                    ("retry", "Retry selected failed downloads"),
                    ("retry all", "Retry all failed or cancelled downloads"),
                ],
                "library" => &[
                    ("music", "Show saved audio"),
                    ("books", "Show saved books"),
                    ("export", "Export the library as JSON"),
                ],
                "account" => &[
                    ("import", "Import Netscape cookies"),
                    ("login udemy", "Open Udemy sign-in"),
                    ("login hotmart", "Open Hotmart sign-in"),
                    ("remove", "Remove a cookie account"),
                ],
                "system" => &[
                    ("doctor", "Check optional dependencies"),
                    ("settings", "Edit paths and concurrency"),
                    ("plugins", "List installed plugins"),
                    ("plugins add", "Register a plugin manifest"),
                    ("worker", "Show worker status"),
                ],
                "help" => &[
                    ("add", "Help for /add"),
                    ("queue", "Help for /queue"),
                    ("library", "Help for /library"),
                    ("convert", "Help for /convert"),
                    ("account", "Help for /account"),
                    ("system", "Help for /system"),
                ],
                _ => &[],
            };
            let rest = rest.trim_start();
            let mut matches: Vec<_> = options
                .iter()
                .filter(|(value, summary)| command_match(rest, value, summary))
                .map(|(value, summary)| (format!("{command} {value}"), *summary))
                .collect();
            matches.sort_by_key(|(value, _)| !value[command.len() + 1..].starts_with(rest));
            return matches;
        }
        let mut matches: Vec<_> = COMMANDS
            .iter()
            .filter(|(_, name, summary)| command_match(input, name, summary))
            .map(|(_, name, summary)| ((*name).to_string(), *summary))
            .collect();
        matches.sort_by_key(|(name, _)| !name.starts_with(input));
        matches
    }
    // ponytail: Ctrl+P palette is a filtered view over existing commands, jobs,
    // files, accounts, and pages. No new navigation concepts.
    fn palette_matches(&self) -> Vec<(String, &'static str, Action)> {
        let query = palette_query(self);
        let mut out: Vec<(String, &'static str, Action)> = Vec::new();
        for (_, name, summary) in COMMANDS {
            if command_match(&query, name, summary) {
                out.push((
                    format!("/{name} "),
                    summary,
                    Action::Fill(format!("/{name} ")),
                ));
            }
        }
        for job in self.snapshot.jobs.iter().take(30) {
            if out.len() >= 14 {
                break;
            }
            if command_match(&query, &job.title, job.status.label()) {
                out.push((
                    job.title.clone(),
                    job.status.label(),
                    Action::QueueJob(job.id.clone()),
                ));
            }
        }
        for job in self
            .snapshot
            .jobs
            .iter()
            .filter(|j| j.status == Status::Completed)
            .take(3)
        {
            if out.len() >= 16 {
                break;
            }
            let label = format!("library {}", job.title);
            if command_match(&query, &label, "Browse completed files") {
                out.push((
                    label.clone(),
                    "Browse completed files",
                    Action::Fill(format!("/library {}", job.title)),
                ));
            }
        }
        for account in self.snapshot.accounts.iter().take(3) {
            if out.len() >= 18 {
                break;
            }
            if command_match(&query, &account.name, "Cookie account") {
                out.push((
                    format!("account {}", account.name),
                    "Cookie account",
                    Action::Page(Page::Accounts),
                ));
            }
        }
        for (label, summary, action) in [
            (
                "system doctor",
                "Check optional dependencies",
                Action::Doctor,
            ),
            (
                "system settings",
                "Edit paths and concurrency",
                Action::Settings,
            ),
            (
                "system worker",
                "Show worker status",
                Action::Page(Page::Tools),
            ),
            (
                "queue issues",
                "Show jobs needing attention",
                Action::Issues,
            ),
        ] {
            if out.len() >= 20 {
                break;
            }
            if command_match(&query, label, summary) {
                out.push((label.to_string(), summary, action));
            }
        }
        out.truncate(10);
        out
    }
    fn palette_open(&self) -> bool {
        self.global && self.composing
    }
    fn open_add(&mut self, source: &str, immediate: bool) -> Result<()> {
        let mut form = self.form(FormKind::Add);
        let source_path = Path::new(source);
        form.fields[0].value = if source_path.is_file()
            && source_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("txt"))
        {
            anyhow::ensure!(
                std::fs::metadata(source_path)?.len() <= 8 * 1024 * 1024,
                "Download list exceeds 8 MB"
            );
            std::fs::read_to_string(source_path)
                .context("Cannot read download list")?
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            source.to_string()
        };
        let specs = form_specs(&form)?;
        self.dialog = Some(Dialog::Form(form));
        if immediate || specs.len() > 1 || specs[0].kind == JobKind::Torrent {
            self.send(Request::Submit(specs), "submitted");
        } else {
            self.send(Request::Inspect(specs[0].clone()), "inspect");
            self.notice = "Inspecting media...".into();
        }
        Ok(())
    }
    fn open_convert(&mut self, source: &str, immediate: bool) -> Result<()> {
        let mut form = self.form(FormKind::Convert);
        form.fields[0].value = source.to_string();
        if immediate && !source.is_empty() {
            let specs = form_specs(&form)?;
            self.dialog = Some(Dialog::Form(form));
            self.send(Request::Submit(specs), "submitted");
        } else {
            self.dialog = Some(Dialog::Form(form));
        }
        Ok(())
    }
    fn queue_control(&mut self, action: Control, all: bool) -> Result<()> {
        let eligible = |status: &Status| match action {
            Control::Pause => matches!(status, Status::Queued | Status::Active),
            Control::Resume => *status == Status::Paused,
            Control::Cancel => matches!(status, Status::Queued | Status::Active | Status::Paused),
            Control::Retry => matches!(status, Status::Failed | Status::Cancelled),
        };
        let requested = if all {
            self.snapshot
                .jobs
                .iter()
                .map(|job| job.id.clone())
                .collect()
        } else {
            self.ids()
        };
        anyhow::ensure!(!requested.is_empty(), "Select a download first");
        let ids: Vec<_> = requested
            .into_iter()
            .filter(|id| {
                self.snapshot
                    .jobs
                    .iter()
                    .find(|job| job.id == *id)
                    .is_some_and(|job| eligible(&job.status))
            })
            .collect();
        anyhow::ensure!(
            !ids.is_empty(),
            "No selected downloads can be {}",
            match action {
                Control::Pause => "paused",
                Control::Resume => "resumed",
                Control::Cancel => "cancelled",
                Control::Retry => "retried",
            }
        );
        if matches!(action, Control::Resume | Control::Retry)
            && self
                .snapshot
                .jobs
                .iter()
                .any(|job| ids.contains(&job.id) && !job.resume_supported && job.downloaded > 0)
        {
            self.dialog = Some(Dialog::Restart { ids, action });
        } else {
            self.send(
                Request::Control {
                    ids,
                    action,
                    allow_restart: false,
                },
                "control",
            );
        }
        Ok(())
    }
    fn run_queue_command(&mut self, rest: &str) -> Result<()> {
        let words: Vec<_> = rest.split_whitespace().collect();
        match words.as_slice() {
            [] => self.action(Action::Page(Page::Downloads)),
            [filter @ ("active" | "queued" | "completed" | "issues")] => {
                self.action(Action::Page(Page::Downloads))?;
                self.issues_only = *filter == "issues";
                self.tab = match *filter {
                    "active" => 1,
                    "queued" => 2,
                    "completed" => 3,
                    _ => 0,
                };
                Ok(())
            }
            [operation @ ("pause" | "resume" | "cancel" | "retry")]
            | [operation @ ("pause" | "resume" | "cancel" | "retry"), "all"] => {
                if self.page != Page::Downloads {
                    self.action(Action::Page(Page::Downloads))?;
                }
                let action = match *operation {
                    "pause" => Control::Pause,
                    "resume" => Control::Resume,
                    "cancel" => Control::Cancel,
                    _ => Control::Retry,
                };
                self.queue_control(action, words.get(1) == Some(&"all"))
            }
            _ => anyhow::bail!(
                "Usage: /queue [active|queued|completed|issues|pause|resume|cancel|retry] [all]"
            ),
        }
    }
    fn run_account_command(&mut self, rest: &str) -> Result<()> {
        let words: Vec<_> = rest.split_whitespace().collect();
        match words.as_slice() {
            [] => self.action(Action::Page(Page::Accounts)),
            ["import"] => self.action(Action::Import),
            ["login", provider @ ("udemy" | "hotmart")] => {
                self.action(Action::Login((*provider).into()))
            }
            ["remove", name] => {
                crate::engine::paths::validate_name(name)?;
                anyhow::ensure!(
                    self.snapshot
                        .accounts
                        .iter()
                        .any(|account| account.name == *name),
                    "Unknown account {name}"
                );
                self.dialog = Some(Dialog::RemoveAccount {
                    name: (*name).into(),
                });
                Ok(())
            }
            _ => anyhow::bail!("Usage: /account [import|login udemy|login hotmart|remove NAME]"),
        }
    }
    fn run_system_command(&mut self, rest: &str) -> Result<()> {
        match rest {
            "" => self.action(Action::Page(Page::Tools)),
            "doctor" => self.action(Action::Doctor),
            "settings" => self.action(Action::Settings),
            "plugins" => {
                self.action(Action::Page(Page::Tools))?;
                self.send(Request::Plugins, "plugins");
                Ok(())
            }
            "plugins add" => self.action(Action::Plugin),
            "worker" => {
                self.info(
                    "Download worker",
                    format!(
                        "Status: {}\nProcess: {}\n\nDownloads continue when the TUI closes. Use /quit to choose whether active transfers keep running.",
                        if self.connected { "Connected" } else { "Reconnecting" },
                        self.snapshot.worker_pid
                    ),
                );
                Ok(())
            }
            _ => anyhow::bail!("Usage: /system [doctor|settings|plugins|plugins add|worker]"),
        }
    }
    fn run_help_command(&mut self, topic: &str) -> Result<()> {
        if topic.is_empty() {
            return self.action(Action::Help);
        }
        let text = match topic {
            "add" => "/add [URL|MAGNET|TORRENT|LIST]\n\nPreview one URL, or queue a list, magnet, or torrent. Pasting a source without /add does the same thing.",
            "queue" => "/queue [active|queued|completed|issues]\n/queue pause|resume|cancel|retry [all]\n\nWithout all, the action applies to marked jobs or the selected job.",
            "library" => "/library [music|books|SEARCH|export]\n\nBrowse, filter, search, or export completed downloads.",
            "convert" => "/convert [FILE]\n\nOpen conversion settings for a local media file.",
            "account" => "/account [import|login udemy|login hotmart|remove NAME]\n\nCookie values are never displayed.",
            "system" => "/system [doctor|settings|plugins|plugins add|worker]\n\nInspect dependencies and configure local tools.",
            "help" => "/help [COMMAND]\n\nShow general or command-specific help.",
            "quit" => "/quit\n\nChoose whether active downloads keep running.",
            _ => anyhow::bail!("Unknown help topic {topic}"),
        };
        self.info(&format!("/{topic}"), text);
        Ok(())
    }
    fn run_composer(&mut self, immediate: bool) -> Result<()> {
        let input = self.composer.trim().to_string();
        anyhow::ensure!(!input.is_empty(), "Paste a URL, local file, or type /");
        let result = (|| {
            let source_path = Path::new(&input);
            if source_path.is_file() {
                let add = source_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| {
                        value.eq_ignore_ascii_case("txt") || value.eq_ignore_ascii_case("torrent")
                    });
                return if add {
                    self.open_add(&input, immediate)
                } else {
                    self.open_convert(&input, immediate)
                };
            }
            if input.starts_with('/') {
                let (command, rest) = parse_command(&input)?;
                match command {
                    SlashCommand::Add => {
                        if rest.is_empty() {
                            self.action(Action::Add)?;
                        } else {
                            self.open_add(rest, immediate)?;
                        }
                    }
                    SlashCommand::Queue => self.run_queue_command(rest)?,
                    SlashCommand::Library => {
                        self.action(Action::Page(Page::Library))?;
                        self.search.clear();
                        match rest {
                            "music" => self.tab = 1,
                            "books" => self.tab = 2,
                            "export" => self.action(Action::Export)?,
                            "" => {}
                            query => self.search = query.to_string(),
                        }
                    }
                    SlashCommand::Convert => self.open_convert(rest, immediate)?,
                    SlashCommand::Account => self.run_account_command(rest)?,
                    SlashCommand::System => self.run_system_command(rest)?,
                    SlashCommand::Help => self.run_help_command(rest)?,
                    SlashCommand::Quit => {
                        anyhow::ensure!(rest.is_empty(), "Usage: /quit");
                        self.action(Action::Quit)?;
                    }
                }
                return Ok(());
            }
            self.open_add(&input, immediate)
        })();
        if result.is_ok() {
            self.composing = false;
            self.global = false;
            self.composer.clear();
        }
        result
    }
    fn info(&mut self, title: &str, text: impl Into<String>) {
        if let Some(Dialog::Form(form)) = &self.dialog {
            self.return_form = Some(form.clone());
        }
        self.dialog = Some(Dialog::Info {
            title: title.into(),
            text: text.into(),
            scroll: 0,
        });
    }
    fn action(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Page(page)=>{self.page=page;self.detail_focus=0;self.tab=0;self.selected=0;self.marked.clear();self.table=TableState::default();self.focus=2;self.issues_only=false;self.search.clear();self.searching=false;}
            Action::Tab(tab)=>{self.tab=tab;self.selected=0;self.table=TableState::default();}
            Action::Row(row)=>{self.selected=row;self.focus=2;}
            Action::Mark(row)=>{self.selected=row;self.focus=2;if let Some(job)=self.visible().get(row){if !self.marked.insert(job.id.clone()){self.marked.remove(&job.id);}}}
            Action::Add=>self.dialog=Some(Dialog::Form(self.form(FormKind::Add))),
            Action::Convert=>self.dialog=Some(Dialog::Form(self.form(FormKind::Convert))),
            Action::Import=>self.dialog=Some(Dialog::Form(self.form(FormKind::Import))),
            Action::Settings=>self.dialog=Some(Dialog::Form(self.form(FormKind::Settings))),
            Action::Plugin=>self.dialog=Some(Dialog::Form(self.form(FormKind::Plugin))),
            Action::Export=>self.dialog=Some(Dialog::Form(self.form(FormKind::Export))),
            Action::Close=>{self.dialog=match self.dialog.take(){Some(Dialog::Info{..})=>self.return_form.take().map(Dialog::Form),Some(Dialog::Playlist{form,..})=>Some(Dialog::Form(form)),_=>None};self.searching=false;}
            Action::Quit=>{if self.snapshot.jobs.iter().any(|j|j.status.pending()){self.dialog=Some(Dialog::Exit{keep:true,focus:0});}else{self.quit=true;}}
            Action::Keep=>{if let Some(Dialog::Exit{keep,..})=&mut self.dialog{*keep = !*keep;}}
            Action::Exit=>{if matches!(self.dialog,Some(Dialog::Exit{keep:false,..})){self.send(Request::PauseAll,"exit");self.notice="Pausing transfers safely…".into();}else{self.quit=true;}}
            Action::Search=>{self.searching=true;self.focus=1;}
            Action::Details=>{if let Some(job)=self.visible().get(self.selected){self.info("Download details",format!("{}\n\nStatus: {}\nSaved: {}\nDestination: {}\n\n{}\n\nFiles\n{}",job.title,job.status.label(),bytes(job.downloaded),job.destination.display(),job.error.as_deref().unwrap_or("No errors reported."),job.files.iter().map(|p|p.display().to_string()).collect::<Vec<_>>().join("\n")));}}
            Action::Pause|Action::Resume|Action::Cancel|Action::Retry=>{
                let control=match action{Action::Pause=>Control::Pause,Action::Resume=>Control::Resume,Action::Retry=>Control::Retry,_=>Control::Cancel};
                self.queue_control(control,false)?;
            }
            Action::Doctor=>{if self.page != Page::Tools { self.action(Action::Page(Page::Tools))?; } self.send(Request::Doctor,"doctor");self.send(Request::Plugins,"plugins");}
            Action::Login(provider)=>{super::open_login(&provider)?;self.info("Finish connecting your account","Sign in in your browser, export Netscape cookies.txt, then choose Import cookies here. Opening the browser alone does not authenticate OmniDownloader.");}
            Action::Field(index)=>{if let Some(Dialog::Form(form))=&mut self.dialog{form.focus=index;}}
            Action::Toggle(index)=>{if let Some(Dialog::Form(form))=&mut self.dialog{form.focus=index;let value=&mut form.fields[index].value;*value=if value=="true"{"false"}else{"true"}.into();}}
            Action::Inspect=>{
                if let Some(Dialog::Form(form))=&self.dialog {let specs=form_specs(form)?;anyhow::ensure!(specs.len()==1,"Inspect one URL at a time");self.send(Request::Inspect(specs[0].clone()),"inspect");self.notice="Inspecting media… you can keep browsing.".into();}
            }
            Action::Submit=>{
                if matches!(self.dialog,Some(Dialog::RemoveAccount{..})){if let Some(Dialog::RemoveAccount{name})=self.dialog.take(){self.send(Request::RemoveAccount(name),"account-removed");return Ok(());}}
                if matches!(self.dialog,Some(Dialog::Restart{..})){if let Some(Dialog::Restart{ids,action})=self.dialog.take(){self.send(Request::Control{ids,action,allow_restart:true},"control");return Ok(());}}
                if let Some(Dialog::Form(form))=self.dialog.clone(){
                    match form.kind {
                        FormKind::Add|FormKind::Convert=>{let specs=form_specs(&form)?;self.send(Request::Submit(specs),"submitted");}
                        FormKind::Import=>self.send(Request::ImportCookies{file:super::absolute(Path::new(form.fields[0].value.trim()))?,name:form.fields[1].value.trim().into()},"account"),
                        FormKind::Settings=>{
                            let mut settings=self.snapshot.settings.clone();settings.output=super::absolute(Path::new(form.fields[0].value.trim()))?;settings.concurrency=form.fields[1].value.trim().parse().context("Enter a number for concurrent downloads")?;
                            for (i,name) in ["yt-dlp","ffmpeg","ffprobe","aria2c","deno"].iter().enumerate(){let value=form.fields[i+2].value.trim();if value.is_empty(){settings.tools.remove(*name);}else{settings.tools.insert(name.to_string(),std::fs::canonicalize(value).with_context(||format!("{name} path does not exist"))?);}}
                            self.send(Request::Settings(settings),"settings");
                        }
                        FormKind::Plugin=>self.send(Request::RegisterPlugin(super::absolute(Path::new(form.fields[0].value.trim()))?),"registered"),
                        FormKind::Export=>{let mut file=std::fs::OpenOptions::new().create_new(true).write(true).open(form.fields[0].value.trim())?;serde_json::to_writer_pretty(&mut file,&super::library(&self.snapshot,&self.search,None))?;self.notice="Library exported.".into();self.dialog=None;}
                    }
                }
            }
            Action::Playlist(index)=>{if let Some(Dialog::Playlist{selected,cursor,..})=&mut self.dialog{*cursor=index;if !selected.insert(index+1){selected.remove(&(index+1));}}}
            Action::PlaylistAll=>{if let Some(Dialog::Playlist{info,selected,..})=&mut self.dialog{if selected.len()==info.entries.len(){selected.clear();}else{*selected=(1..=info.entries.len()).collect();}}}
            Action::ApplyPlaylist=>{if let Some(Dialog::Playlist{selected,mut form,..})=self.dialog.clone(){anyhow::ensure!(!selected.is_empty(),"Select at least one playlist entry");form.fields[6].value=selected.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");self.dialog=Some(Dialog::Form(form));}}
            Action::Help=>self.info("Commands","Type / for commands or Ctrl+P for global search\n\n/add       Download a URL or import a list\n/queue     View and control downloads\n/library   Browse completed files\n/convert   Convert a local media file\n/account   Manage cookie accounts\n/system    Tools, settings, plugins and worker\n/help      Commands and keyboard shortcuts\n/quit      Exit safely\n\nKeyboard\nArrows           Navigate\nEnter            Open or preview\nCtrl+Enter       Queue composer input immediately\nSpace            Select a job\np / r / x        Pause / resume / cancel\nEsc              Back\n\nClosing the terminal window directly leaves pending downloads running."),
            Action::Compose=>{self.composing=true;self.focus=4;self.global=false;}
            Action::ChooseCommand(index)=>{if let Some((value,_))=self.command_matches().get(index){self.composer=format!("/{value} ");self.command_cursor=0;self.composing=true;self.focus=4;}}
            Action::Fill(value)=>{self.composer=value;self.command_cursor=0;self.composing=true;self.global=false;self.focus=4;}
            Action::OpenFolder=>{if let Some(job)=self.visible().get(self.selected){let dir=job.destination.display().to_string();if open_in_file_manager(&job.destination).is_err(){self.info("Download folder",format!("{}\n\n{dir}",job.title));}}}
            Action::CopyPath=>{if let Some(job)=self.visible().get(self.selected){let path=job.files.first().map(|p|p.display().to_string()).unwrap_or_else(||job.destination.display().to_string());if copy_to_clipboard(&path).is_err(){self.info("File path",format!("{path}\n\nSelect and copy it from here."));}}}
            Action::QueueJob(id)=>{self.action(Action::Page(Page::Downloads))?;if let Some(index)=self.visible().iter().position(|job|job.id==id){self.selected=index;self.focus=2;}}
            Action::Issues=>{self.action(Action::Page(Page::Downloads))?;self.issues_only=true;}
        }
        Ok(())
    }
    fn click(&mut self, position: Position) -> Result<()> {
        let action = self
            .hits
            .iter()
            .rev()
            .find(|hit| hit.area.contains(position))
            .map(|hit| hit.action.clone());
        if !matches!(
            action,
            Some(Action::Compose | Action::ChooseCommand(_) | Action::Fill(_))
        ) {
            self.composing = false;
            self.global = false;
        }
        if let Some(action) = action {
            self.action(action)?;
        }
        Ok(())
    }
    fn key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return self.action(Action::Quit);
        }
        if self.dialog.is_some() {
            return self.dialog_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            self.composer = "/".into();
            self.composing = true;
            self.global = true;
            self.focus = 4;
            self.command_cursor = 0;
            return Ok(());
        }
        if self.composing {
            match key.code {
                KeyCode::Esc => {
                    self.composing = false;
                    self.global = false;
                    self.composer.clear();
                }
                KeyCode::Enter => {
                    if self.palette_open() {
                        if let Some((_, _, action)) =
                            self.palette_matches().get(self.command_cursor)
                        {
                            let action = action.clone();
                            self.global = false;
                            self.action(action)?;
                        }
                    } else {
                        let suggestion = self
                            .command_matches()
                            .get(self.command_cursor)
                            .map(|(value, _)| value.clone());
                        if self.composer.starts_with('/')
                            && suggestion
                                .as_ref()
                                .is_some_and(|value| self.composer.trim() != format!("/{value}"))
                        {
                            self.action(Action::ChooseCommand(self.command_cursor))?;
                        } else {
                            self.global = false;
                            self.run_composer(key.modifiers.contains(KeyModifiers::CONTROL))?;
                        }
                    }
                }
                KeyCode::Backspace => {
                    self.composer.pop();
                    self.command_cursor = 0;
                }
                KeyCode::Tab if self.palette_open() => {
                    if let Some((_, _, action)) = self.palette_matches().get(self.command_cursor) {
                        let action = action.clone();
                        self.global = false;
                        self.action(action)?;
                    }
                }
                KeyCode::Tab if self.composer.starts_with('/') => {
                    if let Some((value, _)) = self.command_matches().get(self.command_cursor) {
                        self.composer = format!("/{value} ");
                    }
                }
                KeyCode::Up if self.palette_open() => {
                    self.command_cursor = self.command_cursor.saturating_sub(1)
                }
                KeyCode::Up if self.composer.starts_with('/') => {
                    self.command_cursor = self.command_cursor.saturating_sub(1)
                }
                KeyCode::Down if self.palette_open() => {
                    let len = self.palette_matches().len();
                    self.command_cursor = (self.command_cursor + 1).min(len.saturating_sub(1));
                }
                KeyCode::Down if self.composer.starts_with('/') => {
                    let len = self.command_matches().len();
                    self.command_cursor = (self.command_cursor + 1).min(len.saturating_sub(1));
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.composer.push(c);
                    self.command_cursor = 0;
                }
                _ => {}
            }
            return Ok(());
        }
        if self.searching {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.search.pop();
                }
                KeyCode::Char(c) => self.search.push(c),
                _ => {}
            }
            self.selected = 0;
            return Ok(());
        }
        match key.code {
            KeyCode::Char('q') => self.action(Action::Quit)?,
            KeyCode::Char('?') => self.action(Action::Help)?,
            KeyCode::Char('a') => self.action(Action::Add)?,
            KeyCode::Char('c') => self.action(Action::Convert)?,
            KeyCode::Char('/') => {
                self.composer = "/".into();
                self.action(Action::Compose)?;
            }
            KeyCode::Char('p') => self.action(Action::Pause)?,
            KeyCode::Char('r') => self.action(Action::Resume)?,
            KeyCode::Char('x') => self.action(Action::Cancel)?,
            KeyCode::Tab => {
                // ponytail: one focus cycle everywhere; Downloads action row is the selected card.
                self.focus = match self.focus {
                    2 => 3,
                    3 => 4,
                    _ => 2,
                }
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    4 => 3,
                    3 => 2,
                    _ => 4,
                }
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.visible().len().saturating_sub(1));
            }
            KeyCode::Left => self.detail_focus = self.detail_focus.saturating_sub(1),
            KeyCode::Right => {
                self.detail_focus =
                    (self.detail_focus + 1).min(self.page.actions().len().saturating_sub(1));
            }
            KeyCode::Char(' ') => {
                if let Some(job) = self.visible().get(self.selected) {
                    if !self.marked.insert(job.id.clone()) {
                        self.marked.remove(&job.id);
                    }
                }
            }
            KeyCode::Enter => {
                if self.focus == 4 {
                    self.action(Action::Compose)?;
                } else if self.focus == 3 {
                    if let Some((_, action)) = self.page.actions().get(self.detail_focus) {
                        self.action(action.clone())?;
                    }
                } else {
                    match self.page {
                        Page::Accounts => self.action(Action::Import)?,
                        Page::Tools => self.action(Action::Doctor)?,
                        _ => self.action(Action::Details)?,
                    }
                }
            }
            KeyCode::Esc => {
                self.marked.clear();
                self.search.clear();
            }
            _ => {}
        }
        Ok(())
    }
    fn dialog_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc {
            return self.action(Action::Close);
        }
        let mut action = None;
        match self.dialog.as_mut().unwrap() {
            Dialog::Exit { keep, focus } => match key.code {
                KeyCode::Tab | KeyCode::Right => *focus = (*focus + 1) % 3,
                KeyCode::BackTab | KeyCode::Left => *focus = (*focus + 2) % 3,
                KeyCode::Char(' ') => *keep = !*keep,
                KeyCode::Enter => {
                    action = Some(match *focus {
                        0 => Action::Keep,
                        1 => Action::Close,
                        _ => Action::Exit,
                    })
                }
                _ => {}
            },
            Dialog::Info { scroll, .. } => match key.code {
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Enter => action = Some(Action::Close),
                _ => {}
            },
            Dialog::Restart { .. } => {
                if key.code == KeyCode::Enter {
                    action = Some(Action::Submit)
                }
            }
            Dialog::RemoveAccount { .. } => {
                if key.code == KeyCode::Enter {
                    action = Some(Action::Submit)
                }
            }
            Dialog::Playlist { info, cursor, .. } => match key.code {
                KeyCode::Up => *cursor = cursor.saturating_sub(1),
                KeyCode::Down => *cursor = (*cursor + 1).min(info.entries.len().saturating_sub(1)),
                KeyCode::Char(' ') => action = Some(Action::Playlist(*cursor)),
                KeyCode::Char('a') => action = Some(Action::PlaylistAll),
                KeyCode::Enter => action = Some(Action::ApplyPlaylist),
                _ => {}
            },
            Dialog::Form(form) => {
                let len = form.fields.len();
                let actions = if form.kind == FormKind::Add { 3 } else { 2 };
                match key.code {
                    KeyCode::Tab => {
                        if form.focus < len {
                            form.focus += 1;
                            form.action = 0;
                        } else {
                            form.action += 1;
                            if form.action >= actions {
                                form.focus = 0;
                                form.action = 0;
                            }
                        }
                    }
                    KeyCode::BackTab => {
                        if form.focus == len && form.action > 0 {
                            form.action -= 1;
                        } else if form.focus > 0 {
                            form.focus -= 1;
                        } else {
                            form.focus = len;
                            form.action = actions - 1;
                        }
                    }
                    KeyCode::Down => form.focus = (form.focus + 1).min(len),
                    KeyCode::Up => form.focus = form.focus.saturating_sub(1),
                    KeyCode::Left if form.focus == len => {
                        form.action = form.action.saturating_sub(1)
                    }
                    KeyCode::Right if form.focus == len => {
                        form.action = (form.action + 1).min(actions - 1)
                    }
                    KeyCode::Enter => {
                        if form.focus < len {
                            if form.fields[form.focus].toggle {
                                action = Some(Action::Toggle(form.focus));
                            } else {
                                form.focus += 1;
                            }
                        } else {
                            action = Some(if form.kind == FormKind::Add {
                                [Action::Inspect, Action::Submit, Action::Close][form.action]
                                    .clone()
                            } else {
                                [Action::Submit, Action::Close][form.action].clone()
                            });
                        }
                    }
                    KeyCode::Char(' ') if form.focus < len && form.fields[form.focus].toggle => {
                        action = Some(Action::Toggle(form.focus))
                    }
                    KeyCode::Char(c) if form.focus < len && !form.fields[form.focus].toggle => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'u' {
                            form.fields[form.focus].value.clear();
                        } else if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            form.fields[form.focus].value.push(c);
                        }
                    }
                    KeyCode::Backspace if form.focus < len => {
                        form.fields[form.focus].value.pop();
                    }
                    _ => {}
                }
            }
        }
        if let Some(action) = action {
            self.action(action)?;
        }
        Ok(())
    }
    fn paste(&mut self, text: String) {
        if let Some(Dialog::Form(form)) = &mut self.dialog {
            if let Some(field) = form.fields.get_mut(form.focus) {
                if !field.toggle {
                    field.value.push_str(&text.replace('\r', ""));
                }
            }
        } else if self.searching {
            self.search.push_str(&text);
        } else if self.composing {
            self.composer.push_str(&text.replace('\r', ""));
            self.command_cursor = 0;
        } else {
            self.composer = text.trim().into();
            self.composing = true;
            self.focus = 4;
        }
    }
    fn response(&mut self, tag: String, result: Result<serde_json::Value>) {
        self.busy = false;
        let value = match result {
            Ok(value) => {
                self.connected = true;
                value
            }
            Err(error) => {
                if tag == "snapshot" {
                    self.connected = false;
                    self.notice = "Reconnecting to download worker…".into();
                } else {
                    self.info(
                        "Could not finish that",
                        crate::engine::redact(&format!("{error:#}")),
                    );
                }
                return;
            }
        };
        match tag.as_str() {
            "snapshot" => {
                if let Ok(snapshot) = serde_json::from_value::<Snapshot>(value) {
                    self.snapshot = snapshot;
                    self.selected = self.selected.min(self.visible().len().saturating_sub(1));
                }
            }
            "doctor" => {
                if let Ok(deps) = serde_json::from_value(value) {
                    self.dependencies = deps;
                    self.notice = "Dependency check complete.".into();
                }
            }
            "plugins" => {
                if let Ok(plugins) = serde_json::from_value(value) {
                    self.plugins = plugins;
                }
            }
            "inspect" => {
                if !matches!(self.dialog, Some(Dialog::Form(_))) {
                    return;
                }
                if let Ok(info) = serde_json::from_value::<MediaInfo>(value) {
                    if !info.entries.is_empty() {
                        let form = match self.dialog.take() {
                            Some(Dialog::Form(f)) => f,
                            _ => self.form(FormKind::Add),
                        };
                        let selected = (1..=info.entries.len()).collect();
                        self.dialog = Some(Dialog::Playlist {
                            info,
                            selected,
                            cursor: 0,
                            form,
                        });
                    } else {
                        let duration = info
                            .duration
                            .map(|n| format!("{:02}:{:02}", (n as u64) / 60, (n as u64) % 60))
                            .unwrap_or_else(|| "Unknown length".into());
                        let size = info
                            .size
                            .map(bytes)
                            .unwrap_or_else(|| "Unknown size".into());
                        let formats = if info.formats.is_empty() {
                            "automatic quality".into()
                        } else {
                            info.formats
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" · ")
                        };
                        self.info(
                            "Media preview",
                            format!(
                                "{}\n{duration}  ·  {size}\n{formats}\n{}\n\nEnter back to options · Submit in the form queues it",
                                info.title, info.source,
                            ),
                        );
                    }
                }
            }
            "exit" => self.quit = true,
            "submitted" => {
                self.dialog = None;
                self.page = Page::Downloads;
                self.tab = 0;
                self.selected = 0;
                self.marked.clear();
                self.search.clear();
                self.searching = false;
                self.issues_only = false;
                self.notice = "Added to your queue. Downloads continue in the background.".into();
            }
            "account" => {
                self.dialog = None;
                self.notice =
                    "Cookies imported. The account is ready for supported sources.".into();
            }
            "account-removed" => {
                self.dialog = None;
                self.notice = "Account removed from this profile.".into();
            }
            "settings" => {
                self.dialog = None;
                self.notice = "Settings saved.".into();
                self.send(Request::Doctor, "doctor");
            }
            "registered" => {
                self.dialog = None;
                self.notice = "Plugin registered.".into();
                self.send(Request::Plugins, "plugins");
            }
            _ => self.notice = "Queue updated.".into(),
        }
    }
}

fn form_specs(form: &Form) -> Result<Vec<JobSpec>> {
    let value = |i: usize| form.fields[i].value.trim().to_string();
    if form.kind == FormKind::Convert {
        return Ok(vec![JobSpec {
            source: super::absolute(Path::new(&value(0)))?
                .to_string_lossy()
                .into(),
            kind: JobKind::Convert,
            options: DownloadOptions {
                output: Some(super::absolute(Path::new(&value(1)))?),
                format: Some(value(2)),
                ..Default::default()
            },
        }]);
    }
    let optional = |i| {
        let v = value(i);
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    };
    let options = DownloadOptions {
        output: Some(super::absolute(Path::new(&value(1)))?),
        quality: optional(2)
            .map(|v| v.parse::<u32>())
            .transpose()
            .context("Quality must be a height such as 720 or 1080")?,
        format: optional(3),
        subtitles: optional(4),
        account: optional(5),
        playlist_items: optional(6),
        audio_only: value(7) == "true",
        direct: value(8) == "true",
        ..Default::default()
    };
    let specs: Vec<_> = value(0)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|source| {
            let torrent = source.starts_with("magnet:")
                || Path::new(source)
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("torrent"))
                || url::Url::parse(source)
                    .ok()
                    .and_then(|url| {
                        Path::new(url.path())
                            .extension()
                            .and_then(|value| value.to_str())
                            .map(|value| value.eq_ignore_ascii_case("torrent"))
                    })
                    .unwrap_or(false);
            JobSpec {
                source: source.into(),
                kind: if torrent {
                    JobKind::Torrent
                } else {
                    JobKind::Download
                },
                options: options.clone(),
            }
        })
        .collect();
    anyhow::ensure!(!specs.is_empty(), "Paste at least one URL");
    for spec in &specs {
        crate::engine::store::validate_spec(spec)?;
    }
    Ok(specs)
}

struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}
pub async fn run(paths: Paths, mut client: Client) -> Result<()> {
    let snapshot: Snapshot = client.call(Request::Snapshot).await?;
    let (tx, mut requests) = mpsc::unbounded_channel::<(Request, String)>();
    let (results, mut rx) = mpsc::unbounded_channel::<(String, Result<serde_json::Value>)>();
    let actor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(600));
        loop {
            let (request, tag) = tokio::select! {Some(request)=requests.recv()=>request,_=interval.tick()=>(Request::Snapshot,"snapshot".into()),else=>break};
            if matches!(request, Request::Inspect(_)) {
                let paths = paths.clone();
                let results = results.clone();
                tokio::spawn(async move {
                    let result = async {
                        let mut c = Client::ensure(&paths).await?;
                        c.call::<serde_json::Value>(request).await
                    }
                    .await;
                    let _ = results.send((tag, result));
                });
                continue;
            }
            let result = client.call::<serde_json::Value>(request).await;
            if result.is_err() && tag == "snapshot" {
                if let Ok(new) = Client::ensure(&paths).await {
                    client = new;
                }
            }
            if results.send((tag, result)).is_err() {
                break;
            }
        }
    });
    let mut ui = Ui::new(snapshot, tx);
    ui.send(Request::Doctor, "doctor");
    ui.send(Request::Plugins, "plugins");
    enable_raw_mode()?;
    let _restore = RestoreTerminal;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let outcome: Result<()> = async {
        while !ui.quit {
            while let Ok((tag, result)) = rx.try_recv() {
                ui.response(tag, result);
            }
            terminal.draw(|frame| draw(frame, &mut ui))?;
            if event::poll(Duration::from_millis(20))? {
                let outcome = match event::read()? {
                    Event::Key(key) => ui.key(key),
                    Event::Paste(text) => {
                        ui.paste(text);
                        Ok(())
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            ui.click(Position::new(mouse.column, mouse.row))
                        }
                        MouseEventKind::ScrollDown => {
                            ui.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                        }
                        MouseEventKind::ScrollUp => {
                            ui.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
                        }
                        _ => Ok(()),
                    },
                    _ => Ok(()),
                };
                if let Err(error) = outcome {
                    ui.info("Check your input", format!("{error:#}"));
                }
            }
            tokio::task::yield_now().await;
        }
        Ok(())
    }
    .await;
    actor.abort();
    terminal.show_cursor()?;
    outcome
}

pub fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", value as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if value >= 1024 * 1024 {
        format!("{:.1} MB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.1} KB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

// ponytail: OS launchers via std only. Failures fall back to showing the path.
fn open_in_file_manager(path: &Path) -> Result<()> {
    let target = path.display().to_string();
    #[cfg(windows)]
    std::process::Command::new("explorer")
        .arg(target)
        .spawn()?
        .wait()?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(target)
        .spawn()?
        .wait()?;
    #[cfg(all(not(windows), not(target_os = "macos")))]
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()?
        .wait()?;
    Ok(())
}

// ponytail: clipboard via piped OS utilities, no new crates. Tiny input only.
fn copy_to_clipboard(text: &str) -> Result<()> {
    use std::{io::Write, process::Stdio};
    #[cfg(windows)]
    let mut child = std::process::Command::new("clip")
        .stdin(Stdio::piped())
        .spawn()?;
    #[cfg(target_os = "macos")]
    let mut child = std::process::Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()?;
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut child = std::process::Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .spawn()
        .or_else(|_| {
            std::process::Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(Stdio::piped())
                .spawn()
        })?;
    child
        .stdin
        .take()
        .context("Clipboard pipe unavailable")?
        .write_all(text.as_bytes())?;
    child.wait()?;
    Ok(())
}

fn button(frame: &mut Frame, ui: &mut Ui, area: Rect, label: &str, action: Action, active: bool) {
    let area = area.intersection(frame.area());
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = ui.palette;
    let style = if active {
        Style::default().fg(p.bg).bg(p.cyan).bold()
    } else {
        Style::default().fg(p.fg).bg(p.raised)
    };
    frame.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .style(style),
        area,
    );
    ui.hits.push(Hit { area, action });
}
fn panel(title: &str, p: Palette, focused: bool) -> Block<'_> {
    Block::default()
        .title(
            Line::from(format!(" {title} ")).style(Style::default().fg(if focused {
                p.cyan
            } else {
                p.muted
            })),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(if focused { p.cyan } else { p.line }))
        .style(Style::default().bg(p.panel))
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
fn text(frame: &mut Frame, area: Rect, value: impl Into<String>, color: Color) {
    let area = area.intersection(frame.area());
    frame.render_widget(
        Paragraph::new(value.into())
            .style(Style::default().fg(color))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn logo_pattern(c: char) -> [&'static str; 5] {
    match c {
        'o' => ["111", "101", "101", "101", "111"],
        'm' => ["10001", "11011", "10101", "10001", "10001"],
        'n' => ["1001", "1101", "1011", "1001", "1001"],
        'i' => ["111", "010", "010", "010", "111"],
        'd' => ["110", "101", "101", "101", "110"],
        'w' => ["10001", "10001", "10101", "10101", "01010"],
        'l' => ["100", "100", "100", "100", "111"],
        'a' => ["010", "101", "111", "101", "101"],
        'e' => ["111", "100", "110", "100", "111"],
        'r' => ["110", "101", "110", "101", "101"],
        _ => ["", "", "", "", ""],
    }
}

fn logo_color(p: Palette, letter: usize, row: usize, column: usize, width: usize) -> Color {
    if p.bg == Color::Reset {
        return p.fg;
    }
    if letter >= 4 {
        return p.fg;
    }
    match row {
        0 => Color::Rgb(150, 150, 145),
        4 => Color::Rgb(98, 98, 95),
        _ if column == 0 => Color::Rgb(133, 133, 128),
        _ if column + 1 == width => Color::Rgb(112, 112, 108),
        _ => Color::Rgb(122, 122, 117),
    }
}

fn logo_lines(p: Palette) -> Vec<Line<'static>> {
    let word = "omnidownloader";
    (0..5)
        .map(|row| {
            let mut spans = Vec::new();
            for (letter, c) in word.chars().enumerate() {
                let pattern = logo_pattern(c);
                for column in 0..pattern[0].len() {
                    let filled = pattern[row].as_bytes()[column] == b'1';
                    let color = logo_color(p, letter, row, column, pattern[0].len());
                    spans.push(if filled {
                        if p.bg == Color::Reset {
                            Span::styled("██", Style::default().fg(color))
                        } else {
                            Span::styled("  ", Style::default().bg(color))
                        }
                    } else {
                        Span::styled("  ", Style::default().bg(p.bg))
                    });
                }
                spans.push(Span::styled(" ", Style::default().bg(p.bg)));
            }
            Line::from(spans)
        })
        .collect()
}

fn progress_line(job: &Job, width: u16, p: Palette) -> Line<'static> {
    let percent = if job.status == Status::Completed {
        Some(100.0)
    } else {
        job.total
            .filter(|total| *total > 0)
            .map(|total| (job.downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
    };
    let bar_width = width.saturating_sub(8) as usize;
    let filled = percent
        .map(|value| (value / 100.0 * bar_width as f64).round() as usize)
        .unwrap_or(0)
        .min(bar_width);
    let fill = if p.bg == Color::Reset {
        Span::styled("█".repeat(filled), Style::default().fg(p.fg))
    } else {
        Span::styled(" ".repeat(filled), Style::default().bg(p.fg))
    };
    let suffix = if job.status == Status::Completed {
        " Saved".into()
    } else if let Some(percent) = percent {
        format!(" {percent:>3.0}%")
    } else {
        String::new()
    };
    Line::from(vec![
        fill,
        Span::styled(" ".repeat(bar_width - filled), Style::default().bg(p.bg)),
        Span::styled("|", Style::default().fg(p.fg).bold()),
        Span::styled(suffix, Style::default().fg(p.muted)),
    ])
}

fn draw_activity(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    let jobs = ui.visible();
    let title = if ui.issues_only { "ISSUES" } else { "ACTIVITY" };
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(p.muted).bold()),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if jobs.is_empty() {
        let message = if ui.issues_only {
            "Nothing needs attention."
        } else {
            "No downloads"
        };
        text(
            frame,
            Rect::new(area.x, area.y + 2, area.width, 1),
            message,
            p.muted,
        );
        // ponytail: one shortcut row reusing existing Add/Convert forms. No new concepts.
        if !ui.issues_only && area.height >= 5 {
            let items: [(&str, Action); 4] = [
                ("many links", Action::Add),
                ("open .torrent", Action::Add),
                ("convert file", Action::Convert),
                ("add options", Action::Add),
            ];
            let mut x = area.x;
            for (label, action) in items {
                let width = (label.len() as u16 + 3).min(area.right().saturating_sub(x));
                if width < 5 {
                    break;
                }
                button(
                    frame,
                    ui,
                    Rect::new(x, area.y + 4, width, 1),
                    label,
                    action,
                    false,
                );
                x += width + 1;
            }
        }
        return;
    }
    ui.selected = ui.selected.min(jobs.len() - 1);
    let capacity = (area.height.saturating_sub(2) / 4).max(1) as usize;
    let start = ui.selected.saturating_sub(capacity - 1);
    for (visible_row, (index, job)) in jobs
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .enumerate()
    {
        let y = area.y + 2 + visible_row as u16 * 4;
        let selected = index == ui.selected;
        let row = Rect::new(
            area.x,
            y,
            area.width,
            3.min(area.bottom().saturating_sub(y)),
        );
        if selected {
            frame.render_widget(Block::default().style(Style::default().bg(p.select)), row);
        }
        frame.render_widget(
            Paragraph::new(&*job.title).style(Style::default().fg(p.fg).bold()),
            Rect::new(row.x + 1, row.y, row.width.saturating_sub(2), 1),
        );
        let meta = if job.status == Status::Active {
            let total = job
                .total
                .filter(|t| *t > 0)
                .map(bytes)
                .unwrap_or_else(|| "—".into());
            let eta = job
                .eta
                .map(|s| format!("{}:{:02}", s / 60, s % 60))
                .unwrap_or_else(|| "—".into());
            format!(
                "{}  {}/{}  {}/s  ETA {}",
                job.status.label(),
                bytes(job.downloaded),
                total,
                bytes(job.speed.max(0.0) as u64),
                eta
            )
        } else {
            job.error
                .as_deref()
                .unwrap_or_else(|| job.status.label())
                .to_string()
        };
        text(
            frame,
            Rect::new(row.x + 1, row.y + 1, row.width.saturating_sub(2), 1),
            meta,
            if matches!(job.status, Status::Failed | Status::Cancelled) {
                p.red
            } else if job.status == Status::Paused {
                p.amber
            } else {
                p.muted
            },
        );
        frame.render_widget(
            Paragraph::new(progress_line(job, row.width.saturating_sub(2), p)),
            Rect::new(row.x + 1, row.y + 2, row.width.saturating_sub(2), 1),
        );
        ui.hits.push(Hit {
            area: row,
            action: Action::Row(index),
        });
        // ponytail: status-specific actions share the same handlers as keyboard.
        // They sit on the separator line so cards keep their 4-row rhythm.
        if selected {
            let items: &[(&str, Action)] = match job.status {
                Status::Active => &[
                    ("pause", Action::Pause),
                    ("cancel", Action::Cancel),
                    ("details", Action::Details),
                ],
                Status::Queued => &[
                    ("pause", Action::Pause),
                    ("cancel", Action::Cancel),
                    ("details", Action::Details),
                ],
                Status::Paused => &[
                    ("resume", Action::Resume),
                    ("cancel", Action::Cancel),
                    ("details", Action::Details),
                ],
                Status::Failed | Status::Cancelled => {
                    &[("retry", Action::Retry), ("details", Action::Details)]
                }
                Status::Completed => &[
                    ("details", Action::Details),
                    ("folder", Action::OpenFolder),
                    ("path", Action::CopyPath),
                ],
            };
            let mut x = row.x + 1;
            let y = row.y + 3;
            for (position, (label, action)) in items.iter().enumerate() {
                let width = (*label).len() as u16 + 3;
                if x + width > row.right().saturating_sub(1) {
                    break;
                }
                button(
                    frame,
                    ui,
                    Rect::new(x, y, width, 1),
                    label,
                    action.clone(),
                    ui.focus == 3 && ui.detail_focus == position,
                );
                x += width + 1;
            }
        }
    }
}

fn draw_queue_rail(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(p.line)),
        area,
    );
    let inner = area.inner(Margin::new(2, 0));
    text(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        "QUEUE",
        p.muted,
    );
    let jobs: Vec<_> = ui
        .snapshot
        .jobs
        .iter()
        .filter(|job| matches!(job.status, Status::Active | Status::Queued))
        .collect();
    if jobs.is_empty() {
        text(
            frame,
            Rect::new(inner.x, inner.y + 2, inner.width, 2),
            "Queue is empty.\nType /add to begin.",
            p.muted,
        );
    } else {
        for (row, job) in jobs
            .iter()
            .take((inner.height.saturating_sub(5) / 4) as usize)
            .enumerate()
        {
            let y = inner.y + 2 + row as u16 * 4;
            frame.render_widget(
                Paragraph::new(&*job.title).style(Style::default().fg(p.fg).bold()),
                Rect::new(inner.x, y, inner.width, 1),
            );
            text(
                frame,
                Rect::new(inner.x, y + 1, inner.width, 1),
                if job.status == Status::Active {
                    let total = job
                        .total
                        .filter(|t| *t > 0)
                        .map(bytes)
                        .unwrap_or_else(|| "—".into());
                    format!(
                        "{}/{} · {}/s",
                        bytes(job.downloaded),
                        total,
                        bytes(job.speed.max(0.0) as u64)
                    )
                } else {
                    "Queued".into()
                },
                p.muted,
            );
            frame.render_widget(
                Paragraph::new(progress_line(job, inner.width, p)),
                Rect::new(inner.x, y + 2, inner.width, 1),
            );
            ui.hits.push(Hit {
                area: Rect::new(inner.x, y, inner.width, 3),
                action: Action::QueueJob(job.id.clone()),
            });
        }
    }
    let issues = ui
        .snapshot
        .jobs
        .iter()
        .filter(|job| {
            matches!(
                job.status,
                Status::Paused | Status::Failed | Status::Cancelled
            )
        })
        .count();
    if issues > 0 {
        let area = Rect::new(inner.x, inner.bottom().saturating_sub(2), inner.width, 1);
        text(
            frame,
            area,
            format!("{issues} need attention  /queue issues"),
            p.amber,
        );
        ui.hits.push(Hit {
            area,
            action: Action::Issues,
        });
    }
}

fn draw_composer(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    frame.render_widget(Block::default().style(Style::default().bg(p.panel)), area);
    let value = if ui.composer.is_empty() {
        if ui.composing {
            ">  _".to_string()
        } else {
            "Paste a URL, local file, or type /".to_string()
        }
    } else {
        format!(">  {}{}", ui.composer, if ui.composing { "_" } else { "" })
    };
    text(
        frame,
        Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1),
        value,
        if ui.composer.is_empty() && !ui.composing {
            p.muted
        } else {
            p.fg
        },
    );
    text(
        frame,
        Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
        "Enter preview   Ctrl+Enter queue now   Esc clear",
        p.muted,
    );
    ui.hits.push(Hit {
        area,
        action: Action::Compose,
    });
}

fn command_suggestion_height(ui: &Ui) -> u16 {
    if ui.palette_open() {
        (ui.palette_matches().len().min(5) as u16).saturating_add(1)
    } else if ui.composing && ui.composer.starts_with('/') {
        (ui.command_matches().len().min(5) as u16).saturating_add(1)
    } else {
        0
    }
}

fn draw_suggestion_row(
    frame: &mut Frame,
    ui: &mut Ui,
    row: Rect,
    label: &str,
    summary: &str,
    selected: bool,
    action: Action,
) {
    let p = ui.palette;
    frame.render_widget(
        Paragraph::new(format!("{label:<17} {summary}")).style(
            Style::default()
                .fg(if selected { p.fg } else { p.muted })
                .bg(if selected { p.select } else { p.raised })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
        row,
    );
    ui.hits.push(Hit { area: row, action });
}

fn draw_command_suggestions(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    if ui.palette_open() {
        let matches = ui.palette_matches();
        if matches.is_empty() || area.height < 2 {
            return;
        }
        let popup = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        frame.render_widget(Block::default().style(Style::default().bg(p.raised)), popup);
        let cursor = ui.command_cursor;
        let start = cursor
            .saturating_sub(4)
            .min(matches.len().saturating_sub(5));
        for (row_index, (index, (value, summary, action))) in matches
            .into_iter()
            .enumerate()
            .skip(start)
            .take(5)
            .enumerate()
        {
            let row = Rect::new(popup.x + 1, popup.y + row_index as u16, popup.width - 2, 1);
            draw_suggestion_row(frame, ui, row, &value, summary, index == cursor, action);
        }
        return;
    }
    let matches = ui.command_matches();
    if matches.is_empty() || area.height < 2 {
        return;
    }
    let p = ui.palette;
    let popup = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    frame.render_widget(Block::default().style(Style::default().bg(p.raised)), popup);
    let start = ui
        .command_cursor
        .saturating_sub(4)
        .min(matches.len().saturating_sub(5));
    for (row_index, (index, (value, summary))) in
        matches.iter().enumerate().skip(start).take(5).enumerate()
    {
        let row = Rect::new(popup.x + 1, popup.y + row_index as u16, popup.width - 2, 1);
        draw_suggestion_row(
            frame,
            ui,
            row,
            &format!("/{value}"),
            summary,
            index == ui.command_cursor,
            Action::ChooseCommand(index),
        );
    }
}

fn draw(frame: &mut Frame, ui: &mut Ui) {
    let p = ui.palette;
    let area = frame.area();
    ui.hits.clear();
    frame.render_widget(
        Block::default().style(Style::default().bg(p.bg).fg(p.fg)),
        area,
    );
    if area.width < 42 || area.height < 12 {
        text(
            frame,
            area,
            "OmniDownloader\nEnlarge the terminal to at least 42 x 12.\nPress q to exit.",
            p.fg,
        );
        return;
    }
    let root = area.inner(Margin::new(2, 1));
    let show_logo = ui.page == Page::Downloads && area.height >= 20 && root.width >= 110;
    let zones = Layout::vertical([
        Constraint::Length(if show_logo { 6 } else { 2 }),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .split(root);
    if show_logo {
        frame.render_widget(
            Paragraph::new(logo_lines(p)).alignment(Alignment::Center),
            zones[0],
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("omni", Style::default().fg(p.muted).bold()),
                Span::styled("downloader", Style::default().fg(p.fg).bold()),
            ])),
            zones[0],
        );
    }
    let content = if area.width >= 96 && matches!(ui.page, Page::Downloads | Page::Library) {
        let columns = Layout::horizontal([Constraint::Min(48), Constraint::Length(34)])
            .spacing(2)
            .split(zones[1]);
        draw_queue_rail(frame, ui, columns[1]);
        columns[0]
    } else {
        zones[1]
    };
    let suggestions = command_suggestion_height(ui);
    if ui.page == Page::Downloads {
        let jobs = ui.visible().len() as u16;
        let desired = if jobs == 0 {
            5
        } else {
            2u16.saturating_add(jobs.saturating_mul(4))
        };
        let activity_height = desired
            .min(content.height.saturating_sub(4 + suggestions))
            .max(1);
        let rows = Layout::vertical([
            Constraint::Length(activity_height),
            Constraint::Length(suggestions),
            Constraint::Length(4),
            Constraint::Min(0),
        ])
        .split(content);
        draw_activity(frame, ui, rows[0]);
        draw_command_suggestions(frame, ui, rows[1]);
        draw_composer(frame, ui, rows[2]);
    } else {
        let rows = Layout::vertical([
            Constraint::Min(5),
            Constraint::Length(suggestions),
            Constraint::Length(4),
        ])
        .split(content);
        match ui.page {
            Page::Tools => draw_tools(frame, ui, rows[0]),
            Page::Accounts => draw_accounts(frame, ui, rows[0]),
            Page::Library => draw_downloads(frame, ui, rows[0]),
            Page::Downloads => unreachable!(),
        }
        draw_command_suggestions(frame, ui, rows[1]);
        draw_composer(frame, ui, rows[2]);
    }
    let active = ui
        .snapshot
        .jobs
        .iter()
        .filter(|job| job.status == Status::Active)
        .count();
    let queued = ui
        .snapshot
        .jobs
        .iter()
        .filter(|job| job.status == Status::Queued)
        .count();
    let rate: u64 = ui
        .snapshot
        .jobs
        .iter()
        .map(|job| job.speed.max(0.0) as u64)
        .sum();
    // ponytail: one conditional warning in the existing status line. No setup wizard.
    let missing: Vec<&str> = ui
        .dependencies
        .iter()
        .filter(|d| d.path.is_none())
        .map(|d| d.name.as_str())
        .collect();
    let status = if missing.is_empty() {
        format!(
            "{}  ·  {}  {active} active  {queued} queued  {}/s",
            ui.notice,
            if ui.connected {
                "connected"
            } else {
                "reconnecting"
            },
            bytes(rate)
        )
    } else {
        format!(
            "{}  ·  {} missing  /system doctor  ·  {active} active  {queued} queued",
            ui.notice,
            missing.join(", "),
        )
    };
    text(
        frame,
        Rect::new(zones[2].x, zones[2].y, zones[2].width, 1),
        status,
        if ui.connected && missing.is_empty() {
            p.muted
        } else {
            p.amber
        },
    );
    text(
        frame,
        Rect::new(zones[2].x, zones[2].y + 1, zones[2].width, 1),
        "/queue  /library  /convert  /account  /system     ctrl+p search",
        p.muted,
    );
    if ui.dialog.is_some() {
        ui.hits.clear();
        draw_dialog(frame, ui, area);
    }
}
fn draw_downloads(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    let jobs = ui.visible();
    let detail_height = if area.height >= 22 && area.width >= 62 {
        8
    } else {
        0
    };
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(detail_height),
        Constraint::Length(2),
    ])
    .split(area);
    let subtitle = match ui.page {
        Page::Library => "Everything you have saved, in one place.",
        _ => "Downloads stay available when you leave this screen.",
    };
    text(
        frame,
        rows[0],
        format!("{}   /   {}\n{}", ui.page.label(), jobs.len(), subtitle),
        p.fg,
    );
    let labels = if ui.page == Page::Library {
        vec!["All files", "Music", "Books"]
    } else {
        vec!["All", "Active", "Queued", "Completed"]
    };
    let mut x = rows[1].x;
    for (index, label) in labels.iter().enumerate() {
        let width = (label.len() as u16 + 3).min(rows[1].right().saturating_sub(x));
        button(
            frame,
            ui,
            Rect::new(x, rows[1].y, width, 1),
            label,
            Action::Tab(index),
            ui.tab == index,
        );
        x += width + 1;
    }
    if rows[1].right() > x + 10 {
        let label = if ui.search.is_empty() {
            "Search  /".into()
        } else {
            ui.search.clone()
        };
        button(
            frame,
            ui,
            Rect::new(x + 1, rows[1].y, rows[1].right() - x - 1, 1),
            &label,
            Action::Search,
            ui.searching,
        );
    }
    if ui.searching {
        text(
            frame,
            Rect::new(rows[1].x, rows[1].y + 1, rows[1].width, 1),
            format!("Find: {}_", ui.search),
            p.cyan,
        );
    }
    let table_area = rows[2];
    if jobs.is_empty() {
        let content = table_area.inner(Margin::new(1, 1));
        let (title, body) = if !ui.search.is_empty() {
            (
                "No matching downloads",
                "Try a different title or filename.",
            )
        } else if ui.page == Page::Library {
            ("A home for everything you save","Completed downloads appear here automatically.\nSearch your music, books, videos and files.")
        } else {
            ("Your next download starts here","Paste a link anywhere on this screen, or choose\n+ Add download to set quality and destination.")
        };
        text(frame, content, format!("{title}\n\n{body}"), p.muted);
    } else {
        ui.selected = ui.selected.min(jobs.len() - 1);
        ui.table.select(Some(ui.selected));
        let wide = table_area.width >= 74;
        let row_height = if table_area.height < 10 { 1 } else { 2 };
        let table_rows = jobs.iter().map(|job| {
            let selected = ui.marked.contains(&job.id);
            let percent = job
                .total
                .filter(|t| *t > 0)
                .map(|total| (job.downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
            let color = match job.status {
                Status::Active => p.cyan,
                Status::Completed => p.green,
                Status::Failed => p.red,
                Status::Paused => p.amber,
                _ => p.muted,
            };
            let progress = if job.status == Status::Completed {
                "Saved".into()
            } else if let Some(pc) = percent {
                let count = (pc / 100.0 * 8.0).round() as usize;
                format!(
                    "{}{} {:>3.0}%",
                    "━".repeat(count),
                    "─".repeat(8 - count),
                    pc
                )
            } else {
                job.status.label().into()
            };
            let mut cells = vec![
                Cell::from(if selected { "[x]" } else { "[ ]" }).style(Style::default().fg(p.cyan)),
                Cell::from(job.title.clone()).style(Style::default().fg(p.fg)),
                Cell::from(progress).style(Style::default().fg(color)),
            ];
            if wide {
                cells.push(
                    Cell::from(format!("{}/s", bytes(job.speed.max(0.0) as u64)))
                        .style(Style::default().fg(p.muted)),
                );
                cells.push(
                    Cell::from(
                        job.eta
                            .map(|s| format!("{}:{:02}", s / 60, s % 60))
                            .unwrap_or_else(|| "—".into()),
                    )
                    .style(Style::default().fg(p.muted)),
                );
            }
            Row::new(cells).height(row_height)
        });
        let widths = if wide {
            vec![
                Constraint::Length(3),
                Constraint::Min(15),
                Constraint::Length(14),
                Constraint::Length(10),
                Constraint::Length(6),
            ]
        } else {
            vec![
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(14),
            ]
        };
        let header = if wide {
            vec!["", "TITLE", "PROGRESS", "SPEED", "ETA"]
        } else {
            vec!["", "TITLE", "PROGRESS"]
        };
        let table = Table::new(table_rows, widths)
            .header(
                Row::new(header)
                    .style(Style::default().fg(p.muted))
                    .height(row_height),
            )
            .column_spacing(2)
            .block(panel("Downloads", p, ui.focus == 2))
            .row_highlight_style(Style::default().bg(p.select));
        frame.render_stateful_widget(table, table_area, &mut ui.table);
        let offset = ui.table.offset();
        let first_y = table_area.y + 1 + row_height;
        for index in offset..jobs.len() {
            let y = first_y + (index - offset) as u16 * row_height;
            if y >= table_area.bottom() - 1 {
                break;
            }
            ui.hits.push(Hit {
                area: Rect::new(
                    table_area.x + 1,
                    y,
                    table_area.width.saturating_sub(2),
                    row_height.min(table_area.bottom() - 1 - y),
                ),
                action: Action::Row(index),
            });
            ui.hits.push(Hit {
                area: Rect::new(
                    table_area.x + 1,
                    y,
                    3.min(table_area.width.saturating_sub(2)),
                    1,
                ),
                action: Action::Mark(index),
            });
        }
        if detail_height > 0 {
            if let Some(job) = jobs.get(ui.selected) {
                draw_details(frame, ui, rows[3], job);
            }
        }
    }
    let actions = ui.page.actions();
    let mut x = rows[4].x;
    for (index, (label, action)) in actions.into_iter().enumerate() {
        let width = (label.len() as u16 + 3).min(rows[4].right().saturating_sub(x));
        button(
            frame,
            ui,
            Rect::new(x, rows[4].y + 1, width, 1),
            label,
            action,
            ui.focus == 3 && ui.detail_focus == index,
        );
        x += width + 1;
    }
}
fn draw_details(frame: &mut Frame, ui: &mut Ui, area: Rect, job: &Job) {
    let p = ui.palette;
    frame.render_widget(panel("Selected download", p, ui.focus == 3), area);
    let content = area.inner(Margin::new(2, 1));
    let quality = job
        .spec
        .options
        .quality
        .map(|q| format!("{q}p"))
        .unwrap_or_else(|| "Best available".into());
    let text_value = format!(
        "{}\n{}  ·  {}  ·  {} saved\n{}\n{}",
        job.title,
        job.status.label(),
        quality,
        bytes(job.downloaded),
        job.destination.display(),
        job.error
            .as_deref()
            .unwrap_or("Space selects multiple downloads. Actions apply to all selected items.")
    );
    text(frame, content, text_value, p.muted);
}
fn draw_tools(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    frame.render_widget(panel("Tools & extensions", p, false), area);
    let inner = area.inner(Margin::new(2, 1));
    text(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 2),
        "A small engine room. No silent installations.\nConfigure executable paths in Settings.",
        p.muted,
    );
    let mut y = inner.y + 3;
    for dep in &ui.dependencies {
        if y + 2 >= inner.bottom().saturating_sub(3) {
            break;
        }
        text(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            format!(
                "{}  {}",
                if dep.path.is_some() {
                    "READY  "
                } else {
                    "MISSING"
                },
                dep.name
            ),
            if dep.path.is_some() { p.green } else { p.amber },
        );
        text(
            frame,
            Rect::new(inner.x, y + 1, inner.width, 1),
            dep.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| dep.guidance.clone()),
            p.muted,
        );
        y += 3;
    }
    if y < inner.bottom().saturating_sub(3) {
        text(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            format!(
                "{} installed extensions · CLI: plugins list / plugins run",
                ui.plugins.len()
            ),
            p.muted,
        );
    }
    let y = inner.bottom().saturating_sub(1);
    button(
        frame,
        ui,
        Rect::new(inner.x, y, 14, 1),
        "Check tools",
        Action::Doctor,
        ui.focus == 3 && ui.detail_focus == 0,
    );
    button(
        frame,
        ui,
        Rect::new(inner.x + 16, y, 14, 1),
        "Add plugin",
        Action::Plugin,
        ui.focus == 3 && ui.detail_focus == 1,
    );
}
fn draw_accounts(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    frame.render_widget(panel("Connected accounts", p, false), area);
    let inner = area.inner(Margin::new(2, 1));
    text(frame,Rect::new(inner.x,inner.y,inner.width,3),"Your accounts stay on this computer.\nImport Netscape cookies.txt after signing in.\nCookie values are never shown here.",p.muted);
    let mut y = inner.y + 4;
    if ui.snapshot.accounts.is_empty() {
        text(
            frame,
            Rect::new(inner.x, y, inner.width, 2),
            "No accounts connected yet.\nPublic downloads work without an account.",
            p.muted,
        );
    }
    for account in &ui.snapshot.accounts {
        if y + 2 >= inner.bottom().saturating_sub(5) {
            break;
        }
        text(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            format!(
                "{} · {} cookies · {} expired",
                account.name, account.cookies, account.expired
            ),
            p.fg,
        );
        text(
            frame,
            Rect::new(inner.x, y + 1, inner.width, 1),
            account.domains.join(", "),
            p.muted,
        );
        y += 3;
    }
    let y = inner.bottom().saturating_sub(3);
    button(
        frame,
        ui,
        Rect::new(inner.x, y, 18, 1),
        "Import cookies",
        Action::Import,
        ui.focus == 3 && ui.detail_focus == 0,
    );
    let y = inner.bottom().saturating_sub(1);
    button(
        frame,
        ui,
        Rect::new(inner.x, y, 15, 1),
        "Udemy login",
        Action::Login("udemy".into()),
        ui.focus == 3 && ui.detail_focus == 1,
    );
    button(
        frame,
        ui,
        Rect::new(inner.x + 17, y, 15, 1),
        "Hotmart login",
        Action::Login("hotmart".into()),
        ui.focus == 3 && ui.detail_focus == 2,
    );
}
fn draw_dialog(frame: &mut Frame, ui: &mut Ui, screen: Rect) {
    let p = ui.palette;
    let dialog = ui.dialog.clone().unwrap();
    match dialog {
        Dialog::Exit { keep, focus } => {
            let area = centered(screen, 64, 13);
            frame.render_widget(Clear, area);
            frame.render_widget(panel("Before you go", p, true), area);
            let inner = area.inner(Margin::new(3, 2));
            text(
                frame,
                Rect::new(inner.x, inner.y, inner.width, 3),
                "Downloads are still running.\nChoose what happens when you close this dashboard.",
                p.fg,
            );
            let checkbox = Rect::new(inner.x, inner.y + 3, inner.width, 2);
            text(
                frame,
                checkbox,
                format!(
                    "{} Keep downloads running in the background",
                    if keep { "[x]" } else { "[ ]" }
                ),
                if focus == 0 { p.cyan } else { p.fg },
            );
            ui.hits.push(Hit {
                area: checkbox,
                action: Action::Keep,
            });
            text(
                frame,
                Rect::new(inner.x, inner.y + 5, inner.width, 1),
                if keep {
                    "Reopen OmniDownloader to reconnect."
                } else {
                    "Transfers will pause. Partial files stay safe."
                },
                p.muted,
            );
            let y = area.bottom() - 3;
            button(
                frame,
                ui,
                Rect::new(area.right() - 26, y, 10, 1),
                "Cancel",
                Action::Close,
                focus == 1,
            );
            button(
                frame,
                ui,
                Rect::new(area.right() - 14, y, 10, 1),
                "Exit",
                Action::Exit,
                focus == 2,
            );
        }
        Dialog::Form(form) => {
            let title = match form.kind {
                FormKind::Add => "Add a download",
                FormKind::Convert => "Convert a file",
                FormKind::Import => "Import cookies",
                FormKind::Settings => "Your settings",
                FormKind::Plugin => "Add a trusted plugin",
                FormKind::Export => "Export library",
            };
            let area = centered(
                screen,
                78,
                ((form.fields.len() * 3 + 7) as u16).min(screen.height.saturating_sub(2)),
            );
            frame.render_widget(Clear, area);
            frame.render_widget(panel(title, p, true), area);
            let inner = area.inner(Margin::new(3, 1));
            text(
                frame,
                Rect::new(inner.x, inner.y, inner.width, 1),
                "Tab to move · Ctrl+U clears a field · Esc to cancel",
                p.muted,
            );
            let available = inner.height.saturating_sub(5) as usize / 3;
            let available = available.max(1);
            let offset = if form.focus < form.fields.len() {
                form.focus.saturating_sub(available - 1)
            } else {
                form.fields.len().saturating_sub(available)
            };
            for (i, field) in form.fields.iter().enumerate().skip(offset).take(available) {
                let y = inner.y + 2 + ((i - offset) * 3) as u16;
                let rect = Rect::new(inner.x, y, inner.width, 2);
                let focused = form.focus == i;
                if field.toggle {
                    text(
                        frame,
                        rect,
                        format!(
                            "{} {}",
                            if field.value == "true" { "[x]" } else { "[ ]" },
                            field.label
                        ),
                        if focused { p.cyan } else { p.fg },
                    );
                    ui.hits.push(Hit {
                        area: rect,
                        action: Action::Toggle(i),
                    });
                } else {
                    text(
                        frame,
                        Rect::new(rect.x, rect.y, rect.width, 1),
                        &field.label,
                        if focused { p.cyan } else { p.muted },
                    );
                    let mut value = field.value.replace('\n', " ; ");
                    let max = rect.width.saturating_sub(2) as usize;
                    if value.chars().count() > max {
                        value = value
                            .chars()
                            .rev()
                            .take(max)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                    }
                    if focused {
                        value.push('▏');
                    }
                    frame.render_widget(
                        Paragraph::new(value).style(Style::default().fg(p.fg).bg(if focused {
                            p.select
                        } else {
                            p.raised
                        })),
                        Rect::new(rect.x, rect.y + 1, rect.width, 1),
                    );
                    ui.hits.push(Hit {
                        area: rect,
                        action: Action::Field(i),
                    });
                }
            }
            let y = area.bottom() - 3;
            let mut x = inner.x;
            let actions = if form.kind == FormKind::Add {
                vec![
                    ("Inspect", Action::Inspect),
                    ("Add to queue", Action::Submit),
                    ("Cancel", Action::Close),
                ]
            } else {
                vec![("Save", Action::Submit), ("Cancel", Action::Close)]
            };
            for (index, (label, action)) in actions.into_iter().enumerate() {
                let width = label.len() as u16 + 4;
                button(
                    frame,
                    ui,
                    Rect::new(x, y, width, 1),
                    label,
                    action,
                    form.focus == form.fields.len() && form.action == index,
                );
                x += width + 2;
            }
            if form.fields.len() > available {
                text(
                    frame,
                    Rect::new(inner.x, area.bottom() - 2, inner.width, 1),
                    format!(
                        "Fields {}–{} of {} · Tab reveals more",
                        offset + 1,
                        (offset + available).min(form.fields.len()),
                        form.fields.len()
                    ),
                    p.muted,
                );
            }
        }
        Dialog::Info {
            title,
            text: body,
            scroll,
        } => {
            let area = centered(screen, 82, 25);
            frame.render_widget(Clear, area);
            frame.render_widget(panel(&title, p, true), area);
            let inner = area.inner(Margin::new(3, 2));
            frame.render_widget(
                Paragraph::new(body)
                    .wrap(Wrap { trim: false })
                    .scroll((scroll, 0))
                    .style(Style::default().fg(p.fg)),
                Rect::new(
                    inner.x,
                    inner.y,
                    inner.width,
                    inner.height.saturating_sub(2),
                ),
            );
            button(
                frame,
                ui,
                Rect::new(area.right() - 16, area.bottom() - 3, 12, 1),
                "Close",
                Action::Close,
                true,
            );
        }
        Dialog::Playlist {
            info,
            selected,
            cursor,
            ..
        } => {
            let area = centered(screen, 90, 28);
            frame.render_widget(Clear, area);
            frame.render_widget(panel("Choose playlist entries", p, true), area);
            let inner = area.inner(Margin::new(2, 1));
            text(
                frame,
                Rect::new(inner.x, inner.y, inner.width, 2),
                format!(
                    "{}\n{} of {} selected · Space toggles · a selects all",
                    info.title,
                    selected.len(),
                    info.entries.len()
                ),
                p.fg,
            );
            let visible = inner.height.saturating_sub(5) as usize;
            let start = cursor.saturating_sub(visible.saturating_sub(1));
            for (i, entry) in info.entries.iter().enumerate().skip(start).take(visible) {
                let rect = Rect::new(inner.x, inner.y + 3 + (i - start) as u16, inner.width, 1);
                frame.render_widget(
                    Paragraph::new(format!(
                        "{} {:>3}  {}",
                        if selected.contains(&(i + 1)) {
                            "[x]"
                        } else {
                            "[ ]"
                        },
                        entry.index,
                        entry.title
                    ))
                    .style(
                        Style::default()
                            .fg(if cursor == i { p.cyan } else { p.fg })
                            .bg(if cursor == i { p.select } else { p.panel }),
                    ),
                    rect,
                );
                ui.hits.push(Hit {
                    area: rect,
                    action: Action::Playlist(i),
                });
            }
            let y = area.bottom() - 3;
            button(
                frame,
                ui,
                Rect::new(inner.x, y, 15, 1),
                "Toggle all",
                Action::PlaylistAll,
                false,
            );
            button(
                frame,
                ui,
                Rect::new(inner.x + 17, y, 18, 1),
                "Use selection",
                Action::ApplyPlaylist,
                true,
            );
        }
        Dialog::Restart { .. } => {
            let area = centered(screen, 68, 12);
            frame.render_widget(Clear, area);
            frame.render_widget(panel("Restart required", p, true), area);
            let inner = area.inner(Margin::new(3, 2));
            text(frame,inner,"This transfer cannot safely continue from its last position.\n\nRestart from the beginning? Existing direct-download partial files will be preserved. Conversions regenerate their temporary output.",p.fg);
            button(
                frame,
                ui,
                Rect::new(area.right() - 28, area.bottom() - 3, 12, 1),
                "Cancel",
                Action::Close,
                false,
            );
            button(
                frame,
                ui,
                Rect::new(area.right() - 14, area.bottom() - 3, 11, 1),
                "Restart",
                Action::Submit,
                true,
            );
        }
        Dialog::RemoveAccount { name } => {
            let area = centered(screen, 58, 10);
            frame.render_widget(Clear, area);
            frame.render_widget(panel("Remove account", p, true), area);
            let inner = area.inner(Margin::new(3, 2));
            text(
                frame,
                inner,
                format!(
                    "Remove {name} from this profile?\n\nThe imported cookie file will be deleted."
                ),
                p.fg,
            );
            button(
                frame,
                ui,
                Rect::new(area.right() - 28, area.bottom() - 3, 12, 1),
                "Cancel",
                Action::Close,
                false,
            );
            button(
                frame,
                ui,
                Rect::new(area.right() - 14, area.bottom() - 3, 11, 1),
                "Remove",
                Action::Submit,
                true,
            );
        }
    }
}

pub fn render_preview(path: &Path, width: u16, height: u16) -> Result<()> {
    let snapshot = Snapshot {
        jobs: vec![],
        settings: Settings::default(),
        accounts: vec![],
        worker_pid: 0,
    };
    let (tx, _) = mpsc::unbounded_channel();
    let mut ui = Ui::new(snapshot, tx);
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| draw(f, &mut ui))?;
    let buffer = terminal.backend().buffer();
    let mut output = String::new();
    for y in 0..height {
        for x in 0..width {
            let cell = &buffer[(x, y)];
            if let Color::Rgb(r, g, b) = cell.fg {
                output.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
            }
            if let Color::Rgb(r, g, b) = cell.bg {
                output.push_str(&format!("\x1b[48;2;{r};{g};{b}m"));
            }
            output.push_str(cell.symbol());
        }
        output.push_str("\x1b[0m\n");
    }
    std::fs::write(path, output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(index: usize) -> Job {
        Job {
            id: format!("{index:032}"),
            spec: JobSpec {
                source: "https://example.com/video".into(),
                options: DownloadOptions::default(),
                kind: JobKind::Download,
            },
            title: format!("Video {index} — 下载 {}", "long title ".repeat(20)),
            status: Status::Active,
            downloaded: 50,
            total: Some(100),
            speed: 10.0,
            eta: Some(5),
            destination: "downloads".into(),
            files: vec![],
            error: None,
            created: 0,
            updated: 0,
            resume_supported: true,
            etag: None,
            last_modified: None,
        }
    }

    fn ui() -> (Ui, mpsc::UnboundedReceiver<(Request, String)>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Ui::new(
                Snapshot {
                    jobs: vec![],
                    settings: Settings::default(),
                    accounts: vec![],
                    worker_pid: 0,
                },
                tx,
            ),
            rx,
        )
    }

    #[test]
    fn commands_are_small_and_unambiguous() {
        for (command, name, _) in COMMANDS {
            assert_eq!(parse_command(&format!("/{name}")).unwrap().0, command);
        }
        assert!(parse_command("/doctor").is_err());
        assert!(parse_command("queue").is_err());
        assert_eq!(parse_command("/queue issues").unwrap().1, "issues");
        let (mut ui, _) = ui();
        ui.composer = "/unknown".into();
        ui.composing = true;
        assert!(ui.run_composer(false).is_err());
        assert_eq!(ui.composer, "/unknown");
        assert!(ui.composing);
    }

    #[test]
    fn commands_reuse_existing_pages_and_forms() {
        let (mut ui, _) = ui();
        for (page, index, kind) in [
            (Page::Library, 1, FormKind::Export),
            (Page::Tools, 1, FormKind::Plugin),
            (Page::Accounts, 0, FormKind::Import),
        ] {
            ui.action(Action::Page(page)).unwrap();
            ui.focus = 3;
            ui.detail_focus = index;
            ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap();
            assert!(matches!(&ui.dialog, Some(Dialog::Form(f)) if f.kind == kind));
            ui.action(Action::Close).unwrap();
        }
        ui.composer = "/convert".into();
        ui.run_composer(false).unwrap();
        assert!(matches!(&ui.dialog, Some(Dialog::Form(form)) if form.kind == FormKind::Convert));
        ui.dialog = None;
        ui.composer = "/system settings".into();
        ui.run_composer(false).unwrap();
        assert!(matches!(&ui.dialog, Some(Dialog::Form(form)) if form.kind == FormKind::Settings));
        ui.dialog = None;
        ui.composer = "/queue issues".into();
        ui.run_composer(false).unwrap();
        assert_eq!(ui.page, Page::Downloads);
        assert!(ui.issues_only);
    }

    #[test]
    fn notices_and_page_actions_are_keyboard_and_mouse_accessible() {
        let (mut ui, mut rx) = ui();
        ui.notice = "Dependency check complete.".into();
        ui.composer = "/system doctor".into();
        ui.run_composer(false).unwrap();
        assert_eq!(ui.page, Page::Tools);
        assert!(matches!(rx.try_recv().unwrap().0, Request::Doctor));
        assert!(matches!(rx.try_recv().unwrap().0, Request::Plugins));

        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("Dependency check complete."));
        assert!(ui
            .hits
            .iter()
            .any(|hit| matches!(hit.action, Action::Doctor)));

        ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.focus, 3);
        ui.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(&ui.dialog, Some(Dialog::Form(form)) if form.kind == FormKind::Plugin));

        ui.dialog = None;
        ui.action(Action::Page(Page::Downloads)).unwrap();
        ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.focus, 3);
        ui.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.focus, 4);
    }

    #[test]
    fn autocomplete_uses_the_same_top_level_and_subcommands() {
        let (mut ui, mut rx) = ui();
        ui.composer = "/que".into();
        assert_eq!(ui.command_matches()[0].0, "queue");
        ui.composer = "/queue re".into();
        let matches = ui.command_matches();
        assert!(matches.iter().any(|(value, _)| value == "queue resume"));
        assert!(matches.iter().any(|(value, _)| value == "queue retry"));
        ui.composer = "/system plugins a".into();
        assert_eq!(ui.command_matches()[0].0, "system plugins add");
        ui.composer = "/syt".into();
        assert_eq!(ui.command_matches()[0].0, "system");
        ui.composer = "/system dctr".into();
        assert_eq!(ui.command_matches()[0].0, "system doctor");
        ui.composing = true;
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.composer, "/system doctor ");
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.page, Page::Tools);
        assert!(matches!(rx.try_recv().unwrap().0, Request::Doctor));
        assert!(matches!(rx.try_recv().unwrap().0, Request::Plugins));
    }

    #[test]
    fn mouse_clicks_focus_and_blur_the_composer_without_losing_its_draft() {
        let (mut ui, _) = ui();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let composer = ui
            .hits
            .iter()
            .find(|hit| matches!(hit.action, Action::Compose))
            .unwrap()
            .area;
        ui.click(Position::new(composer.x + 1, composer.y + 1))
            .unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let focused: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(focused.contains(">  _"));
        ui.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.composer, "a");

        ui.click(Position::new(0, 0)).unwrap();
        assert!(!ui.composing);
        ui.key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.composer, "a");

        ui.click(Position::new(composer.x + 1, composer.y + 1))
            .unwrap();
        ui.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
            .unwrap();
        assert_eq!(ui.composer, "ab");

        ui.composer = "/".into();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let moved_composer = ui
            .hits
            .iter()
            .find(|hit| matches!(hit.action, Action::Compose))
            .unwrap()
            .area;
        let suggestion = ui
            .hits
            .iter()
            .find(|hit| matches!(hit.action, Action::ChooseCommand(0)))
            .unwrap()
            .area;
        assert!(suggestion.y >= composer.y);
        assert!(suggestion.bottom() <= moved_composer.y);
        assert!(moved_composer.y > composer.y);
        assert_eq!(suggestion.x, moved_composer.x + 1);
        assert_eq!(suggestion.right(), moved_composer.right() - 1);
        ui.click(Position::new(suggestion.x, suggestion.y)).unwrap();
        assert_eq!(ui.composer, "/add ");
        assert!(ui.composing);
    }

    #[test]
    fn queue_commands_only_target_eligible_jobs() {
        let (mut ui, mut rx) = ui();
        let mut active = job(1);
        active.status = Status::Active;
        let mut paused = job(2);
        paused.status = Status::Paused;
        let mut failed = job(3);
        failed.status = Status::Failed;
        failed.resume_supported = false;
        let mut completed = job(4);
        completed.status = Status::Completed;
        ui.snapshot.jobs = vec![active.clone(), paused, failed, completed];
        ui.composer = "/queue pause all".into();
        ui.run_composer(false).unwrap();
        let (request, _) = rx.try_recv().unwrap();
        assert!(
            matches!(request, Request::Control { ids, action: Control::Pause, .. } if ids == vec![active.id])
        );
        ui.composer = "/queue nonsense".into();
        ui.composing = true;
        assert!(ui.run_composer(false).is_err());
        assert_eq!(ui.composer, "/queue nonsense");
        ui.composer = "/queue issues".into();
        ui.run_composer(false).unwrap();
        ui.selected = 1;
        ui.composer = "/queue retry".into();
        ui.run_composer(false).unwrap();
        assert!(matches!(
            &ui.dialog,
            Some(Dialog::Restart {
                action: Control::Retry,
                ..
            })
        ));
        ui.action(Action::Submit).unwrap();
        let (request, _) = rx.try_recv().unwrap();
        assert!(matches!(
            request,
            Request::Control {
                action: Control::Retry,
                allow_restart: true,
                ..
            }
        ));
    }

    #[test]
    fn account_removal_requires_confirmation() {
        let (mut ui, mut rx) = ui();
        ui.snapshot.accounts.push(Account {
            name: "main".into(),
            domains: vec!["example.com".into()],
            cookies: 1,
            expired: 0,
        });
        ui.composer = "/account remove main".into();
        ui.run_composer(false).unwrap();
        assert!(matches!(&ui.dialog, Some(Dialog::RemoveAccount { .. })));
        assert!(rx.try_recv().is_err());
        ui.action(Action::Submit).unwrap();
        let (request, tag) = rx.try_recv().unwrap();
        assert!(matches!(request, Request::RemoveAccount(name) if name == "main"));
        assert_eq!(tag, "account-removed");
    }

    #[test]
    fn add_detects_torrents_without_an_extra_command() {
        let (ui, _) = ui();
        let mut form = ui.form(FormKind::Add);
        form.fields[0].value = "magnet:?xt=urn:btih:fixture".into();
        assert_eq!(form_specs(&form).unwrap()[0].kind, JobKind::Torrent);
        form.fields[0].value = "https://example.com/file.torrent".into();
        assert_eq!(form_specs(&form).unwrap()[0].kind, JobKind::Torrent);
    }

    #[test]
    fn composer_routes_existing_files_by_type() {
        let (mut ui, mut rx) = ui();
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("clip.mp4");
        std::fs::write(&media, b"fixture").unwrap();
        ui.composer = media.to_string_lossy().into();
        ui.run_composer(false).unwrap();
        assert!(matches!(
            &ui.dialog,
            Some(Dialog::Form(form)) if form.kind == FormKind::Convert
        ));

        ui.dialog = None;
        let torrent = dir.path().join("download.TORRENT");
        std::fs::write(&torrent, b"fixture").unwrap();
        ui.composer = torrent.to_string_lossy().into();
        ui.run_composer(false).unwrap();
        let (request, _) = rx.try_recv().unwrap();
        assert!(matches!(
            request,
            Request::Submit(specs) if specs.len() == 1 && specs[0].kind == JobKind::Torrent
        ));
    }

    #[test]
    fn paste_appends_to_an_active_composer() {
        let (mut ui, _) = ui();
        ui.composer = "/add ".into();
        ui.composing = true;
        ui.command_cursor = 3;
        ui.paste("https://example.com/video\r\n".into());
        assert_eq!(ui.composer, "/add https://example.com/video\n");
        assert_eq!(ui.command_cursor, 0);

        ui.composing = false;
        ui.paste("  https://example.com/other  ".into());
        assert_eq!(ui.composer, "https://example.com/other");
        assert!(ui.composing);
    }

    #[test]
    fn navigation_and_submission_clear_transient_filters() {
        let (mut ui, _) = ui();
        ui.search = "hidden".into();
        ui.searching = true;
        ui.issues_only = true;
        ui.marked.insert("job".into());
        ui.action(Action::Page(Page::Library)).unwrap();
        assert!(ui.search.is_empty());
        assert!(!ui.searching);
        assert!(!ui.issues_only);
        assert!(ui.marked.is_empty());

        ui.search = "hidden".into();
        ui.searching = true;
        ui.issues_only = true;
        ui.selected = 4;
        ui.marked.insert("job".into());
        ui.response("submitted".into(), Ok(serde_json::json!({})));
        assert_eq!(ui.page, Page::Downloads);
        assert_eq!(ui.selected, 0);
        assert!(ui.search.is_empty());
        assert!(!ui.searching);
        assert!(!ui.issues_only);
        assert!(ui.marked.is_empty());
    }

    #[test]
    fn inspection_and_errors_preserve_entered_form_values() {
        let (mut ui, _) = ui();
        ui.action(Action::Add).unwrap();
        ui.paste("https://example.com/video".into());
        ui.response(
            "inspect".into(),
            Ok(serde_json::to_value(MediaInfo {
                title: "Video".into(),
                ..Default::default()
            })
            .unwrap()),
        );
        assert!(matches!(ui.dialog, Some(Dialog::Info { .. })));
        ui.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(
            matches!(&ui.dialog, Some(Dialog::Form(f)) if f.fields[0].value == "https://example.com/video")
        );
        ui.info("Invalid input", "Try again");
        ui.action(Action::Close).unwrap();
        assert!(
            matches!(&ui.dialog, Some(Dialog::Form(f)) if f.fields[0].value == "https://example.com/video")
        );
    }

    #[test]
    fn exit_without_background_waits_for_worker_acknowledgement() {
        let (mut ui, mut rx) = ui();
        ui.dialog = Some(Dialog::Exit {
            keep: false,
            focus: 2,
        });
        ui.action(Action::Exit).unwrap();
        assert!(!ui.quit);
        let (request, tag) = rx.try_recv().unwrap();
        assert!(matches!(request, Request::PauseAll));
        ui.response(tag, Ok(serde_json::json!({"paused":true})));
        assert!(ui.quit);
        let (mut ui, mut rx) = self::ui();
        ui.dialog = Some(Dialog::Exit {
            keep: true,
            focus: 2,
        });
        ui.action(Action::Exit).unwrap();
        assert!(ui.quit);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn pages_and_dialogs_render_at_supported_sizes_without_background_hit_targets() {
        let (mut ui, _) = ui();
        for (width, height) in [
            (1, 1),
            (41, 11),
            (42, 12),
            (60, 20),
            (80, 24),
            (120, 35),
            (180, 50),
        ] {
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            for page in [Page::Downloads, Page::Library, Page::Tools, Page::Accounts] {
                ui.action(Action::Page(page)).unwrap();
                ui.dialog = None;
                terminal.draw(|f| draw(f, &mut ui)).unwrap();
                for action in [
                    Action::Add,
                    Action::Convert,
                    Action::Import,
                    Action::Settings,
                    Action::Help,
                ] {
                    ui.action(action).unwrap();
                    terminal.draw(|f| draw(f, &mut ui)).unwrap();
                    assert!(ui.hits.iter().all(|h| !matches!(
                        h.action,
                        Action::Page(_) | Action::Quit | Action::Compose | Action::Row(_)
                    )));
                    ui.dialog = None;
                }
            }
        }
    }

    #[test]
    fn narrow_scrolled_rows_and_mouse_checkboxes_match_visible_jobs() {
        let (mut ui, _) = ui();
        ui.snapshot.jobs = (0..40).map(job).collect();
        ui.selected = 25;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
        terminal.draw(|f| draw(f, &mut ui)).unwrap();
        let hit = ui
            .hits
            .iter()
            .find(|h| matches!(h.action, Action::Row(25)))
            .unwrap();
        let row: String = (hit.area.x..hit.area.right())
            .map(|x| terminal.backend().buffer()[(x, hit.area.y)].symbol())
            .collect();
        assert!(row.contains("Video 25"), "{row}");
        ui.action(Action::Mark(25)).unwrap();
        ui.action(Action::Mark(26)).unwrap();
        assert_eq!(ui.marked.len(), 2);
        ui.action(Action::Mark(25)).unwrap();
        assert_eq!(ui.marked.len(), 1);
        ui.action(Action::Page(Page::Library)).unwrap();
        assert!(ui.marked.is_empty());
    }

    #[test]
    fn issues_stay_hidden_until_requested() {
        let (mut ui, _) = ui();
        let mut failed = job(1);
        failed.status = Status::Failed;
        let active = job(2);
        ui.snapshot.jobs = vec![failed, active];
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let issue_action = ui
            .hits
            .iter()
            .find(|hit| matches!(hit.action, Action::Issues))
            .unwrap()
            .action
            .clone();
        assert_eq!(
            ui.visible()
                .into_iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            vec![ui.snapshot.jobs[1].id.clone()]
        );
        ui.action(issue_action).unwrap();
        assert_eq!(
            ui.visible()
                .into_iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            vec![ui.snapshot.jobs[0].id.clone()]
        );
    }

    #[test]
    fn composer_moves_down_as_activity_grows() {
        let (mut ui, _) = ui();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        let composer_y = |terminal: &mut Terminal<_>, ui: &mut Ui| {
            terminal.draw(|frame| draw(frame, ui)).unwrap();
            ui.hits
                .iter()
                .find(|hit| matches!(hit.action, Action::Compose))
                .unwrap()
                .area
                .y
        };
        let empty = composer_y(&mut terminal, &mut ui);
        ui.snapshot.jobs.push(job(1));
        let one = composer_y(&mut terminal, &mut ui);
        ui.snapshot.jobs.extend((2..=4).map(job));
        let four = composer_y(&mut terminal, &mut ui);
        assert!(empty < one && one < four, "{empty}, {one}, {four}");
    }

    #[test]
    fn progress_uses_solid_fill_blank_remainder_and_end_cap() {
        let p = Palette::new();
        let line = progress_line(&job(1), 32, p);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(32, 1)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new(line.clone()), frame.area()))
            .unwrap();
        let rendered: String = (0..32)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(rendered.contains('|'));
        assert!(!rendered.chars().any(|c| matches!(c, '░' | '▒' | '▓')));
    }

    #[test]
    fn logo_uses_five_rows_of_seamless_background_cells() {
        let palette = Palette::new();
        let lines = logo_lines(palette);
        assert_eq!(lines.len(), 5);
        assert!(lines.iter().all(|line| line.width() == 110));
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(!rendered.chars().any(|c| matches!(c, '░' | '▒' | '▓')));
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| { span.style.bg.is_some_and(|color| color != palette.bg) }));
    }

    #[test]
    fn empty_state_shows_shortcuts_routing_to_existing_forms() {
        let (mut ui, _) = ui();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("No downloads"));
        assert!(!rendered.contains("Ready when you are"));
        let adds = ui
            .hits
            .iter()
            .filter(|h| matches!(h.action, Action::Add))
            .count();
        assert!(adds >= 3);
        assert!(ui.hits.iter().any(|h| matches!(h.action, Action::Convert)));
        ui.action(Action::Add).unwrap();
        assert!(matches!(&ui.dialog, Some(Dialog::Form(f)) if f.kind == FormKind::Add));
    }

    #[test]
    fn global_palette_finds_jobs_while_slash_stays_command_only() {
        let (mut ui, _) = ui();
        let mut active = job(1);
        active.title = "Unique Lake Footage".into();
        ui.snapshot.jobs = vec![active.clone()];
        ui.composer = "/".into();
        ui.composing = true;
        ui.global = false;
        assert!(ui
            .command_matches()
            .iter()
            .all(|(v, _)| !v.contains("Lake")));
        ui.global = true;
        let palette = ui.palette_matches();
        assert!(palette.iter().any(|(v, _, _)| v.contains("Lake")));
        assert!(palette.iter().any(|(v, _, _)| v.starts_with("/add ")));
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(!ui.global);
    }

    #[test]
    fn selected_cards_expose_status_actions_to_keyboard_and_mouse() {
        let (mut ui, _) = ui();
        let mut failed = job(1);
        failed.status = Status::Failed;
        let mut done = job(2);
        done.status = Status::Completed;
        ui.snapshot.jobs = vec![failed, done];
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        ui.issues_only = true;
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        assert!(ui.hits.iter().any(|h| matches!(h.action, Action::Retry)));
        assert!(ui.hits.iter().any(|h| matches!(h.action, Action::Details)));
        ui.issues_only = false;
        ui.selected = 1;
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        assert!(ui
            .hits
            .iter()
            .any(|h| matches!(h.action, Action::OpenFolder)));
        assert!(ui.hits.iter().any(|h| matches!(h.action, Action::CopyPath)));
        ui.action(Action::CopyPath).unwrap();
        // ponytail: real clipboard when the OS utility exists, info fallback otherwise.
        assert!(ui.dialog.is_none() || matches!(&ui.dialog, Some(Dialog::Info { .. })));
        ui.dialog = None;
        ui.focus = 3;
        ui.detail_focus = 4;
        ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(&ui.dialog, Some(Dialog::Info { .. })));
    }

    #[test]
    fn dependency_warning_only_shows_when_tools_are_missing() {
        let (mut ui, _) = ui();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let clean: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!clean.contains("missing"));
        ui.dependencies = vec![Dependency {
            name: "yt-dlp".into(),
            path: None,
            purpose: "".into(),
            guidance: "".into(),
        }];
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let warned: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(warned.contains("yt-dlp missing"));
        assert!(warned.contains("/system doctor"));
    }

    #[test]
    fn active_cards_show_compact_size_speed_and_eta() {
        let (mut ui, _) = ui();
        let mut active = job(1);
        active.status = Status::Active;
        active.downloaded = 50;
        active.total = Some(100);
        active.speed = 10.0;
        active.eta = Some(65);
        ui.snapshot.jobs = vec![active];
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut ui)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("50 B/100 B"));
        assert!(rendered.contains("ETA 1:05"));
    }
}
