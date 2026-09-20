use omnidownloader::{
    engine::{
        paths::Paths,
        redact, safe_filename,
        store::{parse_cookies, validate_spec, Store},
    },
    protocol::*,
};

fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(dir.path().join("profile"))).unwrap();
    let store = Store::open(paths).unwrap();
    (dir, store)
}
fn spec(output: &std::path::Path) -> JobSpec {
    JobSpec {
        source: "https://example.com/file.pdf".into(),
        kind: JobKind::Download,
        options: DownloadOptions {
            output: Some(output.into()),
            ..Default::default()
        },
    }
}

#[test]
fn submissions_are_atomic_and_collision_free() {
    let (dir, mut store) = setup();
    let ids = store
        .submit(vec![spec(dir.path()), spec(dir.path())])
        .unwrap();
    let jobs = store.jobs().unwrap();
    assert_eq!(jobs.len(), 2);
    assert_ne!(jobs[0].destination, jobs[1].destination);
    assert_ne!(ids[0], ids[1]);
    let mut bad = spec(dir.path());
    bad.source = "file:///private".into();
    assert!(store.submit(vec![spec(dir.path()), bad]).is_err());
    assert_eq!(store.jobs().unwrap().len(), 2);
}
#[test]
fn progress_updates_preserve_queue_order() {
    let (dir, mut store) = setup();
    let ids = store
        .submit(vec![spec(dir.path()), spec(dir.path())])
        .unwrap();
    let mut job = store.get(&ids[0]).unwrap();
    job.downloaded = 99;
    store.put(&job).unwrap();
    assert_eq!(store.jobs().unwrap()[0].id, ids[1]);
}
#[test]
fn crash_recovery_pauses_pending_jobs_and_keeps_partials() {
    let (dir, mut store) = setup();
    let ids = store.submit(vec![spec(dir.path())]).unwrap();
    let mut job = store.get(&ids[0]).unwrap();
    job.status = Status::Active;
    job.downloaded = 1024;
    job.etag = Some("\"v1\"".into());
    store.put(&job).unwrap();
    store.recover().unwrap();
    let job = store.get(&ids[0]).unwrap();
    assert_eq!(job.status, Status::Paused);
    assert_eq!(job.downloaded, 1024);
    assert_eq!(job.etag.as_deref(), Some("\"v1\""));
}
#[test]
fn completed_and_failed_jobs_survive_restart() {
    let (dir, mut store) = setup();
    let ids = store
        .submit(vec![spec(dir.path()), spec(dir.path())])
        .unwrap();
    for (id, status) in ids.iter().zip([Status::Completed, Status::Failed]) {
        let mut job = store.get(id).unwrap();
        job.status = status;
        store.put(&job).unwrap();
    }
    store.recover().unwrap();
    assert_eq!(store.get(&ids[0]).unwrap().status, Status::Completed);
    assert_eq!(store.get(&ids[1]).unwrap().status, Status::Failed);
}
#[test]
fn cookie_parser_preserves_http_only_and_flags_expiry() {
    let cookies="# Netscape HTTP Cookie File\n#HttpOnly_.example.com\tTRUE\t/\tTRUE\t1\tsession\tsecret\n.example.com\tTRUE\t/\tFALSE\t0\tpref\tvalue\n";
    let account = parse_cookies(cookies).unwrap();
    assert_eq!(account.cookies, 2);
    assert_eq!(account.expired, 1);
    assert_eq!(account.domains, vec!["example.com"]);
    assert!(!serde_json::to_string(&account).unwrap().contains("secret"));
}
#[test]
fn invalid_cookie_import_preserves_existing_account() {
    let (dir, store) = setup();
    let source = dir.path().join("cookies.txt");
    std::fs::write(&source, ".example.com\tTRUE\t/\tTRUE\t0\tsession\tsecret\n").unwrap();
    store.import_cookies(&source, "main").unwrap();
    std::fs::write(&source, "invalid").unwrap();
    assert!(store.import_cookies(&source, "main").is_err());
    assert!(
        std::fs::read_to_string(store.paths.account_file("main").unwrap())
            .unwrap()
            .contains("secret")
    );
    assert!(store.import_cookies(&source, "../escape").is_err());
}
#[test]
fn settings_validate_concurrency() {
    let (dir, store) = setup();
    let settings = Settings {
        output: dir.path().into(),
        concurrency: 0,
        ..Default::default()
    };
    assert!(store.save_settings(&settings).is_err());
}
#[test]
fn input_validation_rejects_bad_paths_and_flags() {
    let (dir, _) = setup();
    let mut s = spec(dir.path());
    s.options.filename = Some("../escape".into());
    assert!(validate_spec(&s).is_err());
    s.options.filename = None;
    s.options.playlist_items = Some("--exec evil".into());
    assert!(validate_spec(&s).is_err());
}

#[test]
fn torrent_validation_accepts_supported_sources() {
    let (dir, _) = setup();
    let mut s = spec(dir.path());
    s.kind = JobKind::Torrent;

    s.source = "magnet:?xt=urn:btih:fixture".into();
    validate_spec(&s).unwrap();

    s.source = "https://example.com/file.torrent".into();
    validate_spec(&s).unwrap();

    s.source = "ftp://example.com/file.torrent".into();
    assert!(validate_spec(&s).is_err());

    let local = dir.path().join("FILE.TORRENT");
    std::fs::write(&local, b"fixture").unwrap();
    s.source = local.to_string_lossy().into();
    validate_spec(&s).unwrap();
}

#[test]
fn logs_redact_url_secrets() {
    let value = redact("Failed https://user:password@example.com/file?token=secret#fragment");
    assert!(!value.contains("password"));
    assert!(!value.contains("secret"));
    assert!(value.contains("example.com/file"));
    let nested = redact("request failed (https://user:password@example.com/file?token=secret)\nproxy='socks5://user:password@localhost:1080'");
    assert!(!nested.contains("password"));
    assert!(!nested.contains("secret"));
    assert!(nested.contains('\n'));
}
#[test]
fn filenames_are_portable() {
    assert_eq!(safe_filename("CON.txt"), "_CON.txt");
    assert!(!safe_filename("a/b:c?d").contains('/'));
    assert_eq!(safe_filename("..."), "download");
    assert_eq!(safe_filename("COM9.log"), "_COM9.log");
    assert_eq!(safe_filename("LPT8"), "_LPT8");
    assert!(safe_filename(&"下载".repeat(150)).len() <= 160);
}
#[test]
fn plugin_manifest_requires_declared_capabilities() {
    let (dir, store) = setup();
    let path = dir.path().join("bad.json");
    std::fs::write(&path,r#"{"protocol":1,"id":"bad","name":"Bad","executable":"nothing","capabilities":["unrestricted"]}"#).unwrap();
    assert!(omnidownloader::engine::plugins::register(&store.paths, &path).is_err());
}
