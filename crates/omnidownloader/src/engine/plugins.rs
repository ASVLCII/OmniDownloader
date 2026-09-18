use super::{
    backends,
    paths::{validate_name, Paths},
    worker::SharedStore,
};
use crate::protocol::*;
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub fn list(paths: &Paths) -> Result<Vec<PluginManifest>> {
    let mut result = vec![];
    for entry in std::fs::read_dir(paths.root.join("plugins"))? {
        let file = entry?.path();
        if file.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let manifest: PluginManifest = serde_json::from_slice(&std::fs::read(file)?)?;
        result.push(manifest);
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}
pub fn register(paths: &Paths, file: &Path) -> Result<PluginManifest> {
    let mut manifest: PluginManifest = serde_json::from_slice(&std::fs::read(file)?)?;
    validate_name(&manifest.id)?;
    anyhow::ensure!(
        manifest.protocol == VERSION,
        "Unsupported plugin protocol version"
    );
    anyhow::ensure!(
        !manifest.capabilities.is_empty()
            && manifest
                .capabilities
                .iter()
                .all(|c| ["inspect", "download", "cancel"].contains(&c.as_str())),
        "Plugin capabilities must be inspect, download, or cancel"
    );
    if !manifest.executable.is_absolute() {
        manifest.executable = file
            .parent()
            .unwrap_or(Path::new("."))
            .join(&manifest.executable);
    }
    manifest.executable =
        std::fs::canonicalize(&manifest.executable).context("Plugin executable does not exist")?;
    anyhow::ensure!(
        manifest.executable.is_file(),
        "Plugin executable must be a file"
    );
    let target = paths
        .root
        .join("plugins")
        .join(format!("{}.json", manifest.id));
    std::fs::write(target, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(manifest)
}
fn get(paths: &Paths, id: &str, capability: &str) -> Result<PluginManifest> {
    validate_name(id)?;
    let manifest = list(paths)?
        .into_iter()
        .find(|p| p.id == id)
        .with_context(|| format!("Plugin {id} is not installed"))?;
    anyhow::ensure!(
        manifest.protocol == VERSION && manifest.capabilities.iter().any(|c| c == capability),
        "Plugin does not support {capability}"
    );
    Ok(manifest)
}
struct RequestFile(PathBuf);
impl Drop for RequestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn command(
    paths: &Paths,
    manifest: &PluginManifest,
    value: serde_json::Value,
) -> Result<(Command, RequestFile)> {
    let path = paths.root.join(format!(
        "plugin-request-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&path, serde_json::to_vec(&value)?)?;
    let mut cmd = Command::new(&manifest.executable);
    cmd.args(&manifest.args).arg("--omni-request").arg(&path);
    Ok((cmd, RequestFile(path)))
}
pub async fn inspect(paths: &Paths, id: &str, spec: &JobSpec) -> Result<MediaInfo> {
    let manifest = get(paths, id, "inspect")?;
    let (cmd, _request) = command(
        paths,
        &manifest,
        serde_json::json!({"version":VERSION,"op":"inspect","spec":spec,"data_dir":paths.root}),
    )?;
    let output = backends::capture(cmd, Duration::from_secs(90)).await?;
    let result = String::from_utf8(output)?
        .lines()
        .filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .find(|v| v["type"] == "metadata")
        .context("Plugin did not return metadata")?;
    Ok(serde_json::from_value(result["data"].clone())?)
}
pub async fn execute(
    paths: &Paths,
    id: &str,
    job: &Job,
    store: SharedStore,
    cancel: CancellationToken,
) -> Result<Vec<PathBuf>> {
    let manifest = get(paths, id, "download")?;
    let (cmd, _request) = command(
        paths,
        &manifest,
        serde_json::json!({"version":VERSION,"op":"download","spec":job.spec,"destination":job.destination,"data_dir":paths.root}),
    )?;
    let mut files: Vec<PathBuf> = vec![];
    backends::run_lines(cmd, cancel, |line| {
        let value: serde_json::Value =
            serde_json::from_str(line).context("Plugin emitted invalid JSON")?;
        match value["type"].as_str() {
            Some("progress") => backends::update(&store, &job.id, |j| {
                j.downloaded = value["downloaded"].as_u64().unwrap_or(0);
                j.total = value["total"].as_u64();
                j.speed = value["speed"]
                    .as_f64()
                    .filter(|n| n.is_finite())
                    .unwrap_or(0.0);
            })?,
            Some("complete") => {
                files = serde_json::from_value(value["files"].clone())?;
            }
            Some("error") => {
                anyhow::bail!("{}", value["message"].as_str().unwrap_or("Plugin failed"))
            }
            Some("message") => {}
            _ => anyhow::bail!("Unknown plugin event"),
        }
        Ok(())
    })
    .await?;
    anyhow::ensure!(!files.is_empty(), "Plugin reported no output files");
    let root = std::fs::canonicalize(&job.destination)?;
    for file in &files {
        anyhow::ensure!(
            file.is_file() && std::fs::canonicalize(file)?.starts_with(&root),
            "Plugin output must be a file in its assigned destination"
        );
    }
    Ok(files)
}
