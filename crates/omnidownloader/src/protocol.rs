use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadOptions {
    pub output: Option<PathBuf>,
    pub quality: Option<u32>,
    pub audio_only: bool,
    pub subtitles: Option<String>,
    pub format: Option<String>,
    pub proxy: Option<String>,
    pub account: Option<String>,
    pub playlist_items: Option<String>,
    pub direct: bool,
    pub filename: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub source: String,
    #[serde(default)]
    pub options: DownloadOptions,
    #[serde(default)]
    pub kind: JobKind,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    #[default]
    Download,
    Convert,
    Torrent,
    Plugin(String),
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Active,
    Paused,
    Completed,
    Failed,
    Cancelled,
}
impl Status {
    pub fn terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
    pub fn pending(&self) -> bool {
        matches!(self, Self::Queued | Self::Active)
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Active => "Downloading",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub spec: JobSpec,
    pub title: String,
    pub status: Status,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: f64,
    pub eta: Option<u64>,
    pub destination: PathBuf,
    pub files: Vec<PathBuf>,
    pub error: Option<String>,
    pub created: u64,
    pub updated: u64,
    pub resume_supported: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaInfo {
    pub title: String,
    pub source: String,
    pub duration: Option<f64>,
    pub size: Option<u64>,
    pub formats: Vec<String>,
    pub entries: Vec<MediaEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaEntry {
    pub index: usize,
    pub title: String,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub output: PathBuf,
    pub concurrency: usize,
    pub tools: BTreeMap<String, PathBuf>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            output: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")),
            concurrency: 3,
            tools: BTreeMap::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub path: Option<PathBuf>,
    pub purpose: String,
    pub guidance: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub domains: Vec<String>,
    pub cookies: usize,
    pub expired: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub jobs: Vec<Job>,
    pub settings: Settings,
    pub accounts: Vec<Account>,
    pub worker_pid: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
pub enum Request {
    Snapshot,
    Submit(Vec<JobSpec>),
    Inspect(JobSpec),
    Control {
        ids: Vec<String>,
        action: Control,
        allow_restart: bool,
    },
    PauseAll,
    Stop,
    Doctor,
    Settings(Settings),
    ImportCookies {
        file: PathBuf,
        name: String,
    },
    RemoveAccount(String),
    Plugins,
    RegisterPlugin(PathBuf),
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Pause,
    Resume,
    Cancel,
    Retry,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub token: String,
    pub request: Request,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub version: u32,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}
impl Reply {
    pub fn ok(value: impl Serialize) -> Self {
        Self {
            version: VERSION,
            result: Some(serde_json::to_value(value).expect("serializable response")),
            error: None,
        }
    }
    pub fn error(error: impl ToString) -> Self {
        Self {
            version: VERSION,
            result: None,
            error: Some(error.to_string()),
        }
    }
}
/// Independent subprocess protocol; no Rust ABI or OmniGet binary compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub protocol: u32,
    pub id: String,
    pub name: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub capabilities: Vec<String>,
}
