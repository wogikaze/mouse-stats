use super::*;
#[test]
fn sync_status_distinguishes_local_only_waiting_login_and_active() {
    assert_eq!(idle_sync_status(false, false), Some("ローカルのみ"));
    assert_eq!(idle_sync_status(false, true), Some("ローカルのみ"));
    assert_eq!(idle_sync_status(true, false), Some("ログイン待ち"));
    assert_eq!(idle_sync_status(true, true), None);
}
pub(super) fn memory_runtime(paused: bool) -> Arc<Runtime> {
    let db = db::Db::open(std::path::Path::new(":memory:")).unwrap();
    let device = db.device_id().unwrap();
    let recording = db.start(&device, false).unwrap();
    Arc::new(Runtime {
        db: Mutex::new(db),
        aggregate: Mutex::new(aggregate::Aggregator::new(paused)),
        recording: Mutex::new(recording),
        device,
        health: Mutex::new(Health::default()),
        session: Mutex::new(None),
        pending: Mutex::new(None),
        secret_path: PathBuf::new(),
        identity_path: PathBuf::new(),
        writer_guard: Mutex::new(()),
        after_drain: Mutex::new(None),
        sync_guard: Mutex::new(()),
        sync_requested: AtomicBool::new(false),
        stopping: AtomicBool::new(false),
    })
}
#[test]
fn settings_save_failure_does_not_change_input_collection() {
    let runtime = memory_runtime(false);
    let mut settings = Settings {
        consent: true,
        ..Default::default()
    };
    runtime.persist_settings(&settings).unwrap();
    runtime
        .db
        .lock()
        .unwrap()
        .conn
        .pragma_update(None, "query_only", true)
        .unwrap();
    settings.paused = true;
    assert!(runtime.persist_settings(&settings).is_err());
    assert!(!runtime.aggregate.lock().unwrap().paused);
    assert!(!runtime.db.lock().unwrap().settings().paused);
}
#[test]
fn deletion_cannot_resurrect_a_batch_already_drained_by_the_save_thread() {
    let runtime = memory_runtime(false);
    runtime.aggregate.lock().unwrap().mouse(3, 4, false, 1234);
    let (drained, ready) = std::sync::mpsc::channel();
    let (release, continue_save) = std::sync::mpsc::channel();
    *runtime.after_drain.lock().unwrap() = Some(Box::new(move || {
        drained.send(()).unwrap();
        continue_save.recv().unwrap();
    }));
    let rt = runtime.clone();
    let save = std::thread::spawn(move || rt.flush());
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let (finished, deleted) = std::sync::mpsc::channel();
    let (deleting, started) = std::sync::mpsc::channel();
    let rt = runtime.clone();
    let delete = std::thread::spawn(move || {
        deleting.send(()).unwrap();
        let result = rt.reset_local();
        finished.send(()).unwrap();
        result
    });
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    // Allow deletion to run while the older batch is in flight. A serialized writer waits here.
    let _ = deleted.recv_timeout(Duration::from_millis(100));
    release.send(()).unwrap();
    save.join().unwrap().unwrap();
    delete.join().unwrap().unwrap();
    runtime.flush().unwrap();
    assert!(runtime.db.lock().unwrap().rows().unwrap().is_empty());
    assert!(runtime.health.lock().unwrap().error.is_none());
    runtime.aggregate.lock().unwrap().mouse(0, 1, false, 1234);
    runtime.flush().unwrap();
    assert_eq!(runtime.db.lock().unwrap().rows().unwrap()[0].value, 1000);
}
#[test]
#[cfg(windows)]
fn raw_input_registers_hidden_window_without_collecting_before_consent() {
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{IsWindowVisible, PostMessageW, WM_CLOSE},
    };
    let db = db::Db::open(std::path::Path::new(":memory:")).unwrap();
    let device = db.device_id().unwrap();
    let recording = db.start(&device, false).unwrap();
    let runtime = Arc::new(Runtime {
        db: Mutex::new(db),
        aggregate: Mutex::new(aggregate::Aggregator::new(true)),
        recording: Mutex::new(recording),
        device,
        health: Mutex::new(Health::default()),
        session: Mutex::new(None),
        pending: Mutex::new(None),
        secret_path: PathBuf::new(),
        identity_path: PathBuf::new(),
        writer_guard: Mutex::new(()),
        after_drain: Mutex::new(None),
        sync_guard: Mutex::new(()),
        sync_requested: AtomicBool::new(false),
        stopping: AtomicBool::new(false),
    });
    let hwnd = input::start(runtime.clone())
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    unsafe {
        assert!(!IsWindowVisible(HWND(hwnd as *mut _)).as_bool());
        PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap();
    }
    runtime.flush().unwrap();
    assert!(runtime.db.lock().unwrap().rows().unwrap().is_empty());
    assert!(runtime.health.lock().unwrap().error.is_none());
}
#[test]
#[cfg(windows)]
fn dpapi_survives_atomic_replacement_and_identity_signatures_verify() {
    use base64::Engine;
    use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
    let dir = std::env::temp_dir().join(format!("mouse-stats-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("secret.dpapi");
    secret::save(&path, b"first").unwrap();
    secret::save(&path, b"second").unwrap();
    assert_eq!(secret::load(&path).unwrap(), b"second");
    assert_ne!(std::fs::read(&path).unwrap(), b"second");
    let key = dir.join("identity.dpapi");
    let pubkey = identity::public(&key).unwrap();
    let signature = identity::sign(&key, "a short-lived server challenge").unwrap();
    let public = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(pubkey)
        .unwrap();
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .unwrap();
    let verifier = VerifyingKey::from_sec1_bytes(&public).unwrap();
    assert!(verifier
        .verify(
            b"a short-lived server challenge",
            &Signature::from_slice(&sig).unwrap()
        )
        .is_ok());
    assert!(verifier
        .verify(b"another challenge", &Signature::from_slice(&sig).unwrap())
        .is_err());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(key).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
