use super::paths::Paths;
use crate::protocol::*;
use anyhow::{Context, Result};
use interprocess::local_socket::{
    tokio::{prelude::*, Stream},
    Name,
};
use serde::de::DeserializeOwned;
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub fn socket_name(endpoint: &str) -> std::io::Result<Name<'_>> {
    #[cfg(windows)]
    {
        endpoint.to_ns_name::<interprocess::local_socket::GenericNamespaced>()
    }
    #[cfg(not(windows))]
    {
        endpoint.to_fs_name::<interprocess::local_socket::GenericFilePath>()
    }
}
pub struct Client {
    stream: BufReader<Stream>,
    token: String,
}
impl Client {
    pub async fn connect(paths: &Paths) -> Result<Self> {
        let token = paths.token()?;
        let stream = tokio::time::timeout(
            Duration::from_secs(2),
            Stream::connect(socket_name(&paths.endpoint)?),
        )
        .await??;
        Ok(Self {
            stream: BufReader::new(stream),
            token,
        })
    }
    pub async fn ensure(paths: &Paths) -> Result<Self> {
        if let Ok(client) = Self::connect(paths).await {
            return Ok(client);
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.root.join("worker.log"))?;
        let mut cmd = std::process::Command::new(std::env::current_exe()?);
        cmd.arg("--data-dir")
            .arg(&paths.root)
            .args(["worker", "run"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000008 | 0x00000200 | 0x08000000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let _child = cmd.spawn().context("Could not launch background worker")?;
        for _ in 0..80 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(client) = Self::connect(paths).await {
                return Ok(client);
            }
        }
        anyhow::bail!(
            "Worker did not start. See {}",
            paths.root.join("worker.log").display()
        )
    }
    pub async fn call<T: DeserializeOwned>(&mut self, request: Request) -> Result<T> {
        let envelope = Envelope {
            version: VERSION,
            token: self.token.clone(),
            request,
        };
        let mut data = serde_json::to_vec(&envelope)?;
        data.push(b'\n');
        anyhow::ensure!(data.len() <= MAX_FRAME, "Request too large");
        self.stream.get_mut().write_all(&data).await?;
        let mut line = Vec::new();
        let size = tokio::time::timeout(
            Duration::from_secs(120),
            (&mut self.stream)
                .take(MAX_FRAME as u64)
                .read_until(b'\n', &mut line),
        )
        .await??;
        anyhow::ensure!(
            size > 0 && line.last() == Some(&b'\n'),
            "Worker disconnected or response exceeded size limit"
        );
        let reply: Reply = serde_json::from_slice(&line)?;
        anyhow::ensure!(reply.version == VERSION, "Worker protocol version mismatch");
        if let Some(error) = reply.error {
            anyhow::bail!("{error}");
        }
        Ok(serde_json::from_value(
            reply.result.context("Worker returned no result")?,
        )?)
    }
}
