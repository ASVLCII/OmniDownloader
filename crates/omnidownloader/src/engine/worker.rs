use super::{backends, client::socket_name, now, paths::Paths, plugins, store::Store};
use crate::protocol::*;
use anyhow::Result;
use fs2::FileExt;
use interprocess::local_socket::{
    tokio::{prelude::*, Stream},
    ListenerOptions,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

pub type SharedStore = Arc<Mutex<Store>>;
pub struct State {
    pub store: SharedStore,
    pub active: Mutex<HashMap<String, CancellationToken>>,
    pub stop: CancellationToken,
    pub clients: AtomicUsize,
    pub stopping: AtomicBool,
}

pub async fn run(paths: Paths) -> Result<()> {
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.root.join("worker.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    std::fs::write(paths.root.join("worker.token"), &token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            paths.root.join("worker.token"),
            std::fs::Permissions::from_mode(0o600),
        )?;
    }
    let store = Store::open(paths.clone())?;
    store.recover()?;
    let state = Arc::new(State {
        store: Arc::new(Mutex::new(store)),
        active: Mutex::new(HashMap::new()),
        stop: CancellationToken::new(),
        clients: AtomicUsize::new(0),
        stopping: AtomicBool::new(false),
    });
    let opts = ListenerOptions::new()
        .name(socket_name(&paths.endpoint)?)
        .try_overwrite(true);
    #[cfg(windows)]
    let opts = {
        use interprocess::os::windows::{
            local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
        };
        opts.security_descriptor(SecurityDescriptor::deserialize(
            &widestring::U16CString::from_str("D:P(A;;GA;;;OW)(A;;GA;;;SY)")?,
        )?)
    };
    let listener = opts.create_tokio()?;
    let (done_tx, mut done_rx) = mpsc::channel::<String>(64);
    let mut tick = tokio::time::interval(Duration::from_millis(150));
    let mut idle = Instant::now();
    loop {
        tokio::select! {
            _ = state.stop.cancelled() => break,
            conn = listener.accept() => {
                let conn = conn?; let state = state.clone(); let token = token.clone();
                state.clients.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move { let _ = connection(conn, state.clone(), token).await; state.clients.fetch_sub(1, Ordering::SeqCst); });
            }
            Some(id) = done_rx.recv() => { state.active.lock().unwrap().remove(&id); }
            _ = tick.tick() => {
                let (jobs, settings, paths) = { let store=state.store.lock().unwrap(); (store.jobs()?, store.settings()?, store.paths.clone()) };
                for queued in jobs.iter().rev().filter(|j| j.status == Status::Queued) {
                    // Recheck and claim under the same store lock used by queue controls.
                    // A snapshot must never resurrect a job paused by another client.
                    let claimed = {
                        let store = state.store.lock().unwrap();
                        let mut active = state.active.lock().unwrap();
                        if state.stopping.load(Ordering::SeqCst) || active.len() >= settings.concurrency { break; }
                        let mut job = store.get(&queued.id)?;
                        if job.status != Status::Queued || active.contains_key(&job.id) { continue; }
                        let cancel = CancellationToken::new();
                        job.status = Status::Active; job.error = None; job.updated=now();
                        store.put(&job)?;
                        active.insert(job.id.clone(), cancel.clone());
                        (job, cancel)
                    };
                    let (job, cancel) = claimed;
                    let store=state.store.clone(); let tx=done_tx.clone(); let paths=paths.clone(); let settings=settings.clone();
                    tokio::spawn(async move {
                        let outcome = backends::execute(job.clone(), &paths, &settings, store.clone(), cancel.clone()).await;
                        {
                        let store = store.lock().unwrap();
                        if let Ok(mut current) = store.get(&job.id) {
                            if current.status == Status::Active {
                                current.speed=0.0; current.eta=None; current.updated=now();
                                match outcome {
                                    Ok(files) if !cancel.is_cancelled() => { current.status=Status::Completed; current.files=files; if let Some(total)=current.total { current.downloaded=total; } }
                                    Ok(_) => { current.status=Status::Paused; }
                                    Err(error) => { current.status=if cancel.is_cancelled() { Status::Paused } else { Status::Failed }; current.error=Some(super::redact(&format!("{error:#}"))); }
                                }
                                let _ = store.put(&current);
                            }
                        }
                        }
                        let _ = tx.send(job.id).await;
                    });
                }
                if state.clients.load(Ordering::SeqCst)>0 || !state.active.lock().unwrap().is_empty() || jobs.iter().any(|j| j.status==Status::Queued) { idle=Instant::now(); }
                else if idle.elapsed() > Duration::from_secs(5) { break; }
            }
        }
    }
    pause_all(&state)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.active.lock().unwrap().is_empty() && Instant::now() < deadline {
        if let Ok(Some(id)) = tokio::time::timeout(Duration::from_millis(250), done_rx.recv()).await
        {
            state.active.lock().unwrap().remove(&id);
        }
    }
    drop(listener);
    drop(lock);
    Ok(())
}

async fn connection(conn: Stream, state: Arc<State>, token: String) -> Result<()> {
    let mut stream = BufReader::new(conn);
    loop {
        let mut line = Vec::new();
        let size = (&mut stream)
            .take(MAX_FRAME as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if size == 0 {
            break;
        }
        anyhow::ensure!(line.last() == Some(&b'\n'), "Frame too large");
        let mut stop_after_reply = false;
        let reply = match serde_json::from_slice::<Envelope>(&line) {
            Ok(env) if env.version == VERSION && constant_eq(&env.token, &token) => {
                stop_after_reply = matches!(env.request, Request::Stop);
                match dispatch(env.request, &state).await {
                    Ok(value) => Reply::ok(value),
                    Err(e) => Reply::error(super::redact(&format!("{e:#}"))),
                }
            }
            _ => Reply::error("Invalid protocol version or authentication token"),
        };
        let mut data = serde_json::to_vec(&reply)?;
        data.push(b'\n');
        stream.get_mut().write_all(&data).await?;
        if stop_after_reply {
            state.stop.cancel();
            break;
        }
    }
    Ok(())
}
fn constant_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0, |acc, (a, b)| acc | (a ^ b))
        == 0
}

async fn dispatch(request: Request, state: &Arc<State>) -> Result<serde_json::Value> {
    use serde_json::json;
    anyhow::ensure!(
        !state.stopping.load(Ordering::SeqCst)
            || matches!(request, Request::Snapshot | Request::Stop),
        "Worker is stopping; reconnect after shutdown"
    );
    match request {
        Request::Snapshot => Ok(serde_json::to_value(
            state.store.lock().unwrap().snapshot()?,
        )?),
        Request::Submit(specs) => Ok(json!(state.store.lock().unwrap().submit(specs)?)),
        Request::Inspect(spec) => {
            super::store::validate_spec(&spec)?;
            let (paths, settings) = {
                let store = state.store.lock().unwrap();
                (store.paths.clone(), store.settings()?)
            };
            Ok(serde_json::to_value(
                backends::inspect(&spec, &paths, &settings).await?,
            )?)
        }
        Request::Control {
            ids,
            action,
            allow_restart,
        } => {
            let store = state.store.lock().unwrap();
            let mut jobs = ids
                .iter()
                .map(|id| store.get(id))
                .collect::<Result<Vec<_>>>()?;
            for job in &jobs {
                if matches!(action, Control::Resume | Control::Retry) {
                    anyhow::ensure!(
                        job.status != Status::Completed,
                        "Completed jobs cannot be resumed; create a new download"
                    );
                    anyhow::ensure!(job.resume_supported || allow_restart || job.downloaded==0, "{} must restart from the beginning. Confirm restart or use --allow-restart.",job.title);
                }
            }
            for job in &mut jobs {
                match action {
                    Control::Pause | Control::Cancel => {
                        if job.status == Status::Completed {
                            continue;
                        }
                        job.status = if matches!(action, Control::Pause) {
                            Status::Paused
                        } else {
                            Status::Cancelled
                        };
                        if let Some(cancel) = state.active.lock().unwrap().get(&job.id) {
                            cancel.cancel();
                        }
                    }
                    Control::Resume | Control::Retry => {
                        if job.status == Status::Active {
                            continue;
                        }
                        if !job.resume_supported && allow_restart {
                            job.downloaded = 0;
                        }
                        job.status = Status::Queued;
                        job.error = None;
                    }
                }
                job.speed = 0.0;
                job.updated = now();
                store.put(job)?;
            }
            Ok(json!({"updated":ids}))
        }
        Request::PauseAll => {
            pause_all(state)?;
            wait_active(state).await?;
            Ok(json!({"paused":true}))
        }
        Request::Stop => {
            state.stopping.store(true, Ordering::SeqCst);
            pause_all(state)?;
            wait_active(state).await?;
            Ok(json!({"stopped":true}))
        }
        Request::Doctor => {
            let store = state.store.lock().unwrap();
            Ok(serde_json::to_value(backends::doctor(&store.settings()?))?)
        }
        Request::Settings(settings) => {
            state.store.lock().unwrap().save_settings(&settings)?;
            Ok(json!(settings))
        }
        Request::ImportCookies { file, name } => Ok(json!(state
            .store
            .lock()
            .unwrap()
            .import_cookies(&file, &name)?)),
        Request::RemoveAccount(name) => {
            let file = state.store.lock().unwrap().paths.account_file(&name)?;
            std::fs::remove_file(file)?;
            Ok(json!({"removed":name}))
        }
        Request::Plugins => {
            let store = state.store.lock().unwrap();
            Ok(json!(plugins::list(&store.paths)?))
        }
        Request::RegisterPlugin(file) => {
            let store = state.store.lock().unwrap();
            Ok(json!(plugins::register(&store.paths, &file)?))
        }
    }
}
fn pause_all(state: &State) -> Result<()> {
    let store = state.store.lock().unwrap();
    for mut job in store.jobs()? {
        if job.status.pending() {
            job.status = Status::Paused;
            job.speed = 0.0;
            store.put(&job)?;
        }
    }
    for cancel in state.active.lock().unwrap().values() {
        cancel.cancel();
    }
    Ok(())
}
async fn wait_active(state: &State) -> Result<()> {
    for _ in 0..100 {
        if state.active.lock().unwrap().is_empty() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("Transfers have not stopped yet; retry after cleanup finishes")
}
