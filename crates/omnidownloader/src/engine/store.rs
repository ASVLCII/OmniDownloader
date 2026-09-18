use super::{now, paths::Paths, safe_filename};
use crate::protocol::*;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::{collections::BTreeSet, path::Path};
pub struct Store {
    conn: Connection,
    pub paths: Paths,
}
impl Store {
    pub fn open(paths: Paths) -> Result<Self> {
        let conn = Connection::open(paths.root.join("state.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS jobs (id TEXT PRIMARY KEY, body TEXT NOT NULL); CREATE TABLE IF NOT EXISTS config (key TEXT PRIMARY KEY, body TEXT NOT NULL); PRAGMA user_version=1;")?;
        Ok(Self { conn, paths })
    }
    pub fn settings(&self) -> Result<Settings> {
        let mut q = self
            .conn
            .prepare("SELECT body FROM config WHERE key='settings'")?;
        let mut rows = q.query([])?;
        match rows.next()? {
            Some(row) => Ok(serde_json::from_str(&row.get::<_, String>(0)?)?),
            None => Ok(Settings::default()),
        }
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        anyhow::ensure!(
            (1..=16).contains(&settings.concurrency),
            "Concurrency must be between 1 and 16"
        );
        anyhow::ensure!(
            settings.output.is_absolute(),
            "Output directory must be absolute"
        );
        self.conn.execute(
            "INSERT OR REPLACE INTO config(key, body) VALUES ('settings', ?1)",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }
    pub fn jobs(&self) -> Result<Vec<Job>> {
        let mut q = self
            .conn
            .prepare("SELECT body FROM jobs ORDER BY rowid DESC")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|s| Ok(serde_json::from_str(&s?)?)).collect()
    }
    pub fn get(&self, id: &str) -> Result<Job> {
        let body: String = self
            .conn
            .query_row("SELECT body FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .with_context(|| format!("Unknown job {id}"))?;
        Ok(serde_json::from_str(&body)?)
    }
    pub fn put(&self, job: &Job) -> Result<()> {
        self.conn.execute("INSERT INTO jobs(id, body) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", params![job.id, serde_json::to_string(job)?])?;
        Ok(())
    }
    pub fn submit(&mut self, specs: Vec<JobSpec>) -> Result<Vec<String>> {
        anyhow::ensure!(
            !specs.is_empty() && specs.len() <= 500,
            "Submit between 1 and 500 jobs"
        );
        let settings = self.settings()?;
        let mut jobs = Vec::new();
        for spec in specs {
            validate_spec(&spec)?;
            let id = uuid::Uuid::new_v4().simple().to_string();
            let title = title_for(&spec);
            let root = spec
                .options
                .output
                .clone()
                .unwrap_or_else(|| settings.output.clone());
            anyhow::ensure!(root.is_absolute(), "Output directory must be absolute");
            let destination = root.join(format!("{}-{}", safe_filename(&title), &id[..8]));
            let resume_supported = matches!(spec.kind, JobKind::Download | JobKind::Torrent);
            jobs.push(Job {
                id,
                spec,
                title,
                status: Status::Queued,
                downloaded: 0,
                total: None,
                speed: 0.0,
                eta: None,
                destination,
                files: vec![],
                error: None,
                created: now(),
                updated: now(),
                resume_supported,
                etag: None,
                last_modified: None,
            });
        }
        let tx = self.conn.transaction()?;
        for job in &jobs {
            tx.execute(
                "INSERT INTO jobs(id,body) VALUES (?1,?2)",
                params![job.id, serde_json::to_string(job)?],
            )?;
        }
        tx.commit()?;
        Ok(jobs.into_iter().map(|j| j.id).collect())
    }
    pub fn recover(&self) -> Result<()> {
        for mut job in self.jobs()? {
            if job.status.pending() {
                job.status = Status::Paused;
                job.speed = 0.0;
                job.error = Some("Worker stopped. Resume this job when ready.".into());
                self.put(&job)?;
            }
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            jobs: self.jobs()?,
            settings: self.settings()?,
            accounts: self.accounts()?,
            worker_pid: std::process::id(),
        })
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut result = vec![];
        for entry in std::fs::read_dir(self.paths.root.join("accounts"))? {
            let path = entry?.path();
            if path.extension().and_then(|x| x.to_str()) == Some("txt") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(mut account) = parse_cookies(&text) {
                        account.name = path.file_stem().unwrap().to_string_lossy().into_owned();
                        result.push(account);
                    }
                }
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
    pub fn import_cookies(&self, file: &Path, name: &str) -> Result<Account> {
        anyhow::ensure!(
            std::fs::metadata(file)?.len() <= 8 * 1024 * 1024,
            "Cookie file exceeds 8 MB"
        );
        let text = std::fs::read_to_string(file)?;
        let mut account = parse_cookies(&text)?;
        account.name = name.to_string();
        std::fs::write(self.paths.account_file(name)?, text)?;
        Ok(account)
    }
}
pub fn validate_spec(spec: &JobSpec) -> Result<()> {
    anyhow::ensure!(!spec.source.trim().is_empty(), "Source cannot be empty");
    if spec.kind == JobKind::Convert {
        anyhow::ensure!(
            Path::new(&spec.source).is_file(),
            "Conversion input does not exist"
        );
    } else if spec.kind == JobKind::Torrent && spec.source.starts_with("magnet:") {
    } else if spec.kind == JobKind::Torrent
        && Path::new(&spec.source).extension().and_then(|x| x.to_str()) == Some("torrent")
    {
        anyhow::ensure!(
            Path::new(&spec.source).is_file(),
            "Torrent file does not exist"
        );
    } else {
        let url = url::Url::parse(&spec.source).context("Enter an http:// or https:// URL")?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https"),
            "Only HTTP and HTTPS URLs are supported for this job"
        );
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "URL credentials are not supported; use an account"
        );
    }
    if let Some(ref account) = spec.options.account {
        super::paths::validate_name(account)?;
    }
    if let Some(ref proxy) = spec.options.proxy {
        let p = url::Url::parse(proxy)?;
        anyhow::ensure!(
            matches!(p.scheme(), "http" | "https" | "socks5"),
            "Unsupported proxy scheme"
        );
    }
    if let Some(ref name) = spec.options.filename {
        anyhow::ensure!(
            !name.is_empty() && safe_filename(name) == *name,
            "Filename must be a plain safe filename"
        );
    }
    if let Some(q) = spec.options.quality {
        anyhow::ensure!((1..=16384).contains(&q), "Invalid quality height");
    }
    if let Some(ref items) = spec.options.playlist_items {
        anyhow::ensure!(
            !items.is_empty()
                && items
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ',' || c == '-'),
            "Playlist selection must use indices/ranges such as 1,3-5"
        );
    }
    if let Some(ref format) = spec.options.format {
        anyhow::ensure!(
            ["mp4", "mkv", "webm", "mp3", "m4a", "flac", "wav", "opus", "jpg", "png"]
                .contains(&format.as_str()),
            "Unsupported format"
        );
    }
    Ok(())
}
fn title_for(spec: &JobSpec) -> String {
    if spec.kind == JobKind::Convert {
        return Path::new(&spec.source)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    url::Url::parse(&spec.source)
        .ok()
        .map(|u| {
            let tail = u
                .path_segments()
                .and_then(|mut p| p.next_back())
                .unwrap_or("");
            if tail.is_empty() {
                u.host_str().unwrap_or("Download").to_string()
            } else {
                tail.to_string()
            }
        })
        .unwrap_or_else(|| "Download".into())
}
pub fn parse_cookies(text: &str) -> Result<Account> {
    let mut domains = BTreeSet::new();
    let mut cookies = 0;
    let mut expired = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('#') && !line.starts_with("#HttpOnly_") {
            continue;
        }
        let fields: Vec<_> = line.trim_start_matches("#HttpOnly_").split('\t').collect();
        anyhow::ensure!(
            fields.len() == 7,
            "Expected Netscape cookies.txt with seven tab-separated fields"
        );
        let expiry: u64 = fields[4].parse().context("Invalid cookie expiry")?;
        anyhow::ensure!(
            !fields[0].is_empty() && !fields[5].is_empty(),
            "Cookie domain and name are required"
        );
        domains.insert(fields[0].trim_start_matches('.').to_string());
        cookies += 1;
        if expiry != 0 && expiry < now() {
            expired += 1;
        }
    }
    anyhow::ensure!(cookies > 0, "No cookies found");
    Ok(Account {
        name: String::new(),
        domains: domains.into_iter().collect(),
        cookies,
        expired,
    })
}
