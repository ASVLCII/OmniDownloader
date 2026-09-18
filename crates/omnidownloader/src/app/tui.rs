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
    Courses,
    Convert,
    Tools,
    Accounts,
    Settings,
}
const PAGES: [Page; 7] = [
    Page::Downloads,
    Page::Library,
    Page::Courses,
    Page::Convert,
    Page::Tools,
    Page::Accounts,
    Page::Settings,
];
impl Page {
    fn actions(self) -> Vec<(&'static str, Action)> {
        match self {
            Self::Library => vec![
                ("Details", Action::Details),
                ("Export library", Action::Export),
            ],
            Self::Courses => vec![
                ("Add course", Action::Add),
                ("Import cookies", Action::Import),
                ("Details", Action::Details),
            ],
            Self::Convert => vec![
                ("Convert a file", Action::Convert),
                ("Details", Action::Details),
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
            Self::Settings => vec![("Edit settings", Action::Settings)],
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
            Self::Courses => "Courses",
            Self::Convert => "Convert",
            Self::Tools => "Tools",
            Self::Accounts => "Accounts",
            Self::Settings => "Settings",
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
            bg: Color::Rgb(11, 16, 23),
            panel: Color::Rgb(16, 23, 33),
            raised: Color::Rgb(23, 33, 46),
            fg: Color::Rgb(225, 234, 243),
            muted: Color::Rgb(129, 148, 168),
            line: Color::Rgb(42, 57, 74),
            cyan: Color::Rgb(70, 213, 222),
            green: Color::Rgb(105, 209, 157),
            amber: Color::Rgb(239, 193, 112),
            red: Color::Rgb(241, 128, 136),
            select: Color::Rgb(24, 61, 73),
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
                    Page::Convert => j.spec.kind == JobKind::Convert,
                    Page::Courses => {
                        j.spec.source.contains("udemy.com")
                            || j.spec.source.contains("hotmart.com")
                            || matches!(&j.spec.kind,JobKind::Plugin(id) if id=="telegram")
                    }
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
                    match self.tab {
                        1 => j.status == Status::Active,
                        2 => j.status == Status::Queued || j.status == Status::Paused,
                        3 => j.status == Status::Completed,
                        _ => true,
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
            Action::Page(page)=>{self.page=page;self.detail_focus=0;self.tab=0;self.selected=0;self.marked.clear();self.table=TableState::default();self.focus=2;}
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
                let ids=self.ids();if ids.is_empty(){self.notice="Select a download first.".into();return Ok(());}
                let control=match action{Action::Pause=>Control::Pause,Action::Resume=>Control::Resume,Action::Retry=>Control::Retry,_=>Control::Cancel};
                if matches!(control,Control::Resume|Control::Retry)&&self.snapshot.jobs.iter().any(|j|ids.contains(&j.id)&&!j.resume_supported&&j.downloaded>0){self.dialog=Some(Dialog::Restart{ids});}
                else{self.send(Request::Control{ids,action:control,allow_restart:false},"control");}
            }
            Action::Doctor=>{self.send(Request::Doctor,"doctor");self.send(Request::Plugins,"plugins");}
            Action::Login(provider)=>{super::open_login(&provider)?;self.info("Finish connecting your account","Sign in in your browser, export Netscape cookies.txt, then choose Import cookies here. Opening the browser alone does not authenticate OmniDownloader.");}
            Action::Field(index)=>{if let Some(Dialog::Form(form))=&mut self.dialog{form.focus=index;}}
            Action::Toggle(index)=>{if let Some(Dialog::Form(form))=&mut self.dialog{form.focus=index;let value=&mut form.fields[index].value;*value=if value=="true"{"false"}else{"true"}.into();}}
            Action::Inspect=>{
                if let Some(Dialog::Form(form))=&self.dialog {let specs=form_specs(form)?;anyhow::ensure!(specs.len()==1,"Inspect one URL at a time");self.send(Request::Inspect(specs[0].clone()),"inspect");self.notice="Inspecting media… you can keep browsing.".into();}
            }
            Action::Submit=>{
                if matches!(self.dialog,Some(Dialog::Restart{..})){if let Some(Dialog::Restart{ids})=self.dialog.take(){self.send(Request::Control{ids,action:Control::Resume,allow_restart:true},"control");return Ok(());}}
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
            Action::Help=>self.info("Make yourself at home","Mouse\nClick navigation, tabs, rows, fields and buttons. Scroll lists with the wheel.\n\nKeyboard\nTab / Shift+Tab  Change focus\nArrows           Navigate\nEnter            Activate / inspect selected item\nSpace            Select a job or toggle a checkbox\na                Add download\nc                Convert a file\np / r / x        Pause / resume / cancel selected jobs\n/                Search\n?                This guide\nq / Ctrl+C       Exit dialog\nEsc              Back\n\nSelect several jobs with Space, then pause, resume or cancel them together.\nClosing the terminal window directly leaves pending downloads running."),
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
            KeyCode::Char('/') => self.action(Action::Search)?,
            KeyCode::Char('p') => self.action(Action::Pause)?,
            KeyCode::Char('r') => self.action(Action::Resume)?,
            KeyCode::Char('x') => self.action(Action::Cancel)?,
            KeyCode::Tab => self.focus = (self.focus + 1) % 5,
            KeyCode::BackTab => self.focus = (self.focus + 4) % 5,
            KeyCode::Up => {
                if self.focus == 0 {
                    let index = PAGES.iter().position(|p| *p == self.page).unwrap_or(0);
                    self.action(Action::Page(PAGES[index.saturating_sub(1)]))?;
                    self.focus = 0;
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if self.focus == 0 {
                    let index = PAGES.iter().position(|p| *p == self.page).unwrap_or(0);
                    self.action(Action::Page(PAGES[(index + 1).min(6)]))?;
                    self.focus = 0;
                } else {
                    self.selected = (self.selected + 1).min(self.visible().len().saturating_sub(1));
                }
            }
            KeyCode::Left => {
                if self.focus == 1 {
                    self.action(Action::Tab(self.tab.saturating_sub(1)))?;
                } else if self.focus == 3 {
                    self.detail_focus = self.detail_focus.saturating_sub(1);
                }
            }
            KeyCode::Right => {
                if self.focus == 1 {
                    self.action(Action::Tab(
                        (self.tab + 1).min(if self.page == Page::Library { 2 } else { 3 }),
                    ))?;
                } else if self.focus == 3 {
                    self.detail_focus = (self.detail_focus + 1).min(self.page.actions().len() - 1);
                }
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
                    self.action(Action::Add)?;
                } else if self.focus == 3 {
                    if let Some((_, action)) = self.page.actions().get(self.detail_focus) {
                        self.action(action.clone())?;
                    }
                } else {
                    match self.page {
                        Page::Settings => self.action(Action::Settings)?,
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
        } else {
            let mut form = self.form(FormKind::Add);
            form.fields[0].value = text.trim().into();
            self.dialog = Some(Dialog::Form(form));
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
                        self.info(
                            "Media preview",
                            format!(
                                "{}\n\nDuration: {}\nSize: {}\n\nFormats\n{}",
                                info.title,
                                info.duration
                                    .map(|n| format!("{n:.0} seconds"))
                                    .unwrap_or_else(|| "Unknown".into()),
                                info.size.map(bytes).unwrap_or_else(|| "Unknown".into()),
                                info.formats.join("\n")
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
                self.notice = "Added to your queue. Downloads continue in the background.".into();
            }
            "account" => {
                self.dialog = None;
                self.notice =
                    "Cookies imported. The account is ready for supported sources.".into();
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
        .map(|source| JobSpec {
            source: source.into(),
            kind: JobKind::Download,
            options: options.clone(),
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
                            let hit = ui
                                .hits
                                .iter()
                                .rev()
                                .find(|h| h.area.contains(Position::new(mouse.column, mouse.row)))
                                .map(|h| h.action.clone());
                            if let Some(action) = hit {
                                ui.action(action)
                            } else {
                                Ok(())
                            }
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
        .border_type(BorderType::Rounded)
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
            "OmniDownloader\nEnlarge the terminal to at least 42 × 12.\nPress q to exit.",
            p.fg,
        );
        return;
    }
    let root = area.inner(Margin::new(2, 1));
    let zones = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(root);
    let header = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(20),
        Constraint::Length(4),
    ])
    .spacing(2)
    .split(zones[0]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("OMNI", Style::default().fg(p.cyan).bold()),
                Span::styled("DOWNLOADER", Style::default().fg(p.fg).bold()),
            ]),
            Line::from(Span::styled(
                "Your downloads. Your space.",
                Style::default().fg(p.muted),
            )),
        ]),
        header[0],
    );
    button(
        frame,
        ui,
        Rect::new(header[1].x, header[1].y, header[1].width, 1),
        "+ Add download",
        Action::Add,
        ui.focus == 4,
    );
    button(
        frame,
        ui,
        Rect::new(header[2].x, header[2].y, header[2].width, 1),
        " ? ",
        Action::Help,
        false,
    );
    let main = if area.width >= 82 {
        let columns = Layout::horizontal([Constraint::Length(20), Constraint::Min(10)])
            .spacing(2)
            .split(zones[1]);
        draw_sidebar(frame, ui, columns[0]);
        columns[1]
    } else {
        let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).split(zones[1]);
        let width = (rows[0].width / 7).max(1);
        for (i, page) in PAGES.iter().enumerate() {
            button(
                frame,
                ui,
                Rect::new(
                    rows[0].x + i as u16 * width,
                    rows[0].y,
                    width.saturating_sub(1),
                    1,
                ),
                &page.label()[..4],
                Action::Page(*page),
                *page == ui.page,
            );
        }
        rows[1]
    };
    match ui.page {
        Page::Tools => draw_tools(frame, ui, main),
        Page::Accounts => draw_accounts(frame, ui, main),
        Page::Settings => draw_settings(frame, ui, main),
        _ => draw_downloads(frame, ui, main),
    }
    let count = ui
        .snapshot
        .jobs
        .iter()
        .filter(|j| j.status == Status::Active)
        .count();
    let queued = ui
        .snapshot
        .jobs
        .iter()
        .filter(|j| j.status == Status::Queued)
        .count();
    let rate = ui
        .snapshot
        .jobs
        .iter()
        .map(|j| j.speed.max(0.0) as u64)
        .sum();
    let status = if ui.connected {
        "Connected"
    } else {
        "Reconnecting"
    };
    text(
        frame,
        Rect::new(zones[2].x, zones[2].y, zones[2].width.saturating_sub(9), 1),
        format!(
            "{}  ·  {count} active  ·  {queued} queued  ·  {}/s",
            status,
            bytes(rate)
        ),
        if ui.connected { p.green } else { p.amber },
    );
    button(
        frame,
        ui,
        Rect::new(zones[2].right().saturating_sub(8), zones[2].y, 8, 1),
        "Quit  q",
        Action::Quit,
        false,
    );
    text(
        frame,
        Rect::new(zones[2].x, zones[2].y + 1, zones[2].width, 1),
        &ui.notice,
        p.muted,
    );
    text(
        frame,
        zones[3],
        "Tab focus   Enter open   Space select   / search   ? help",
        p.muted,
    );
    if ui.dialog.is_some() {
        ui.hits.clear();
        draw_dialog(frame, ui, area);
    }
}
fn draw_sidebar(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    frame.render_widget(Block::default().style(Style::default().bg(p.panel)), area);
    text(
        frame,
        Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(4), 1),
        "WORKSPACE",
        p.muted,
    );
    for (i, page) in PAGES.iter().enumerate() {
        let row = Rect::new(
            area.x + 1,
            area.y + 3 + i as u16 * 2,
            area.width.saturating_sub(2),
            1,
        );
        if row.y >= area.bottom().saturating_sub(1) {
            break;
        }
        let selected = *page == ui.page;
        let label = format!("{} {}", if selected { "▎" } else { " " }, page.label());
        frame.render_widget(
            Paragraph::new(label).style(
                Style::default()
                    .fg(if selected { p.cyan } else { p.muted })
                    .bg(if selected { p.select } else { p.panel })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            row,
        );
        ui.hits.push(Hit {
            area: row,
            action: Action::Page(*page),
        });
    }
    if area.height > 21 {
        let done = ui
            .snapshot
            .jobs
            .iter()
            .filter(|j| j.status == Status::Completed)
            .count();
        text(
            frame,
            Rect::new(
                area.x + 2,
                area.bottom() - 4,
                area.width.saturating_sub(4),
                3,
            ),
            format!(
                "{} saved\nLocal. Independent.\nv{}",
                done,
                env!("CARGO_PKG_VERSION")
            ),
            p.muted,
        );
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
        Page::Courses => "Udemy: experimental · Hotmart adapter planned",
        Page::Convert => "Transform media without leaving your terminal.",
        _ => "A little less waiting. A lot more saved.",
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
        frame.render_widget(panel("Your queue", p, ui.focus == 2), table_area);
        let content = table_area.inner(Margin::new(3, 2));
        let (title, body) = if !ui.search.is_empty() {
            (
                "No matching downloads",
                "Try a different title or filename.",
            )
        } else if ui.page == Page::Library {
            ("A home for everything you save","Completed downloads appear here automatically.\nSearch your music, books, videos and files.")
        } else if ui.page == Page::Courses {
            ("Course downloads","Udemy uses experimental yt-dlp routing with imported cookies.\nHotmart and Telegram account adapters are planned.\nLive course access has not been verified.")
        } else if ui.page == Page::Convert {
            ("Give your media a new format","Convert a video, extract audio, or save a still image.\nYour original file stays intact.")
        } else {
            ("Your next download starts here","Paste a link anywhere on this screen, or choose\n+ Add download to set quality and destination.")
        };
        text(frame, content, format!("\n{title}\n\n{body}"), p.muted);
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
        false,
    );
    button(
        frame,
        ui,
        Rect::new(inner.x + 16, y, 14, 1),
        "Add plugin",
        Action::Plugin,
        false,
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
        true,
    );
    let y = inner.bottom().saturating_sub(1);
    button(
        frame,
        ui,
        Rect::new(inner.x, y, 15, 1),
        "Udemy login",
        Action::Login("udemy".into()),
        false,
    );
    button(
        frame,
        ui,
        Rect::new(inner.x + 17, y, 15, 1),
        "Hotmart login",
        Action::Login("hotmart".into()),
        false,
    );
}
fn draw_settings(frame: &mut Frame, ui: &mut Ui, area: Rect) {
    let p = ui.palette;
    frame.render_widget(panel("Settings", p, false), area);
    let inner = area.inner(Margin::new(2, 1));
    let settings = &ui.snapshot.settings;
    text(frame,inner,format!("Make room for your workflow.\n\nDefault destination\n{}\n\nConcurrent downloads\n{} at a time\n\nBackground downloads\nContinue by default. Choose otherwise in the exit dialog.\n\nAppearance\nDark slate + cyan. Set NO_COLOR for monochrome.\n\nNo automatic startup. No silent dependency updates.",settings.output.display(),settings.concurrency),p.muted);
    button(
        frame,
        ui,
        Rect::new(inner.x, inner.bottom().saturating_sub(1), 18, 1),
        "Edit settings",
        Action::Settings,
        true,
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
    fn keyboard_activates_the_visible_page_action() {
        let (mut ui, _) = ui();
        for (page, index, kind) in [
            (Page::Library, 1, FormKind::Export),
            (Page::Convert, 0, FormKind::Convert),
            (Page::Tools, 1, FormKind::Plugin),
            (Page::Accounts, 0, FormKind::Import),
            (Page::Settings, 0, FormKind::Settings),
        ] {
            ui.action(Action::Page(page)).unwrap();
            ui.focus = 3;
            ui.detail_focus = index;
            ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap();
            assert!(matches!(&ui.dialog, Some(Dialog::Form(f)) if f.kind == kind));
            ui.action(Action::Close).unwrap();
        }
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
            for page in PAGES {
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
                    assert!(ui
                        .hits
                        .iter()
                        .all(|h| !matches!(h.action, Action::Page(_) | Action::Quit)));
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
}
