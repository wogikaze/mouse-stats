mod aggregate;
mod db;
mod identity;
mod input;
mod model;
#[cfg(test)]
mod native_tests;
mod secret;
mod sync;
use model::*;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_deep_link::DeepLinkExt;

#[derive(Default)]
pub struct Health {
    error: Option<String>,
    sync_error: Option<String>,
    sync_status: String,
    generation_conflict: bool,
    input_ready: bool,
}
pub struct Runtime {
    db: Mutex<db::Db>,
    aggregate: Mutex<aggregate::Aggregator>,
    recording: Mutex<String>,
    device: String,
    health: Mutex<Health>,
    session: Mutex<Option<sync::Session>>,
    pending: Mutex<Option<sync::Pending>>,
    secret_path: PathBuf,
    pending_path: PathBuf,
    identity_path: PathBuf,
    writer_guard: Mutex<()>,
    #[cfg(test)]
    after_drain: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    sync_guard: Mutex<()>,
    sync_requested: AtomicBool,
    stopping: AtomicBool,
}
fn idle_sync_status(sync_enabled: bool, signed_in: bool) -> Option<&'static str> {
    if !sync_enabled {
        Some("ローカルのみ")
    } else if !signed_in {
        Some("ログイン待ち")
    } else {
        None
    }
}
impl Runtime {
    // Caller holds sync_guard so an authentication callback cannot race disconnection.
    fn prepare_disconnect(&self, new_device: Option<&str>) -> Result<(), String> {
        let _writer = self.writer_guard.lock().map_err(|_| "Writer lock")?;
        self.flush_locked()?;
        self.db
            .lock()
            .map_err(|_| "DB lock")?
            .disconnect(new_device)?;
        *self.session.lock().map_err(|_| "Session lock")? = None;
        *self.pending.lock().map_err(|_| "Auth lock")? = None;
        let _ = std::fs::remove_file(&self.pending_path);
        let mut health = self.health.lock().map_err(|_| "Health lock")?;
        health.generation_conflict = false;
        health.sync_error = None;
        health.sync_status = "ローカルのみ".into();
        Ok(())
    }
    fn persist_settings(&self, settings: &Settings) -> Result<(), String> {
        self.db
            .lock()
            .map_err(|_| "DB lock")?
            .save_settings(settings)?;
        self.aggregate.lock().map_err(|_| "Input lock")?.paused =
            !settings.consent || settings.paused;
        Ok(())
    }
    fn error(&self, error: &str) {
        if let Ok(mut h) = self.health.lock() {
            h.error = Some(error.into());
        }
    }
    fn flush(&self) -> Result<(), String> {
        let _writer = self.writer_guard.lock().map_err(|_| "Writer lock")?;
        self.flush_locked()
    }
    fn flush_locked(&self) -> Result<(), String> {
        if self
            .aggregate
            .lock()
            .map_err(|_| "Input lock")?
            .clock_warning
        {
            self.error("システム時計の大きな変更を検出しました。時間帯別の集計を確認してください");
        }
        let recording = self.recording.lock().map_err(|_| "Recording lock")?.clone();
        let rows = self
            .aggregate
            .lock()
            .map_err(|_| "Input lock")?
            .drain(&recording);
        if rows.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        {
            let hook = self
                .after_drain
                .lock()
                .map_err(|_| "Test hook lock")?
                .take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let result = self
            .db
            .lock()
            .map_err(|_| "DB lock")?
            .flush(&rows)
            .map_err(|e| e.to_string());
        if result.is_err() {
            self.aggregate
                .lock()
                .map_err(|_| "Input lock")?
                .restore(rows);
            self.error("集計値の保存に失敗しました");
        }
        result
    }
    fn reset_local(&self) -> Result<(), String> {
        let _writer = self.writer_guard.lock().map_err(|_| "Writer lock")?;
        self.flush_locked()?;
        let id = self.db.lock().map_err(|_| "DB lock")?.reset_recording(
            &self.device,
            None,
            false,
            true,
        )?;
        *self.recording.lock().map_err(|_| "Recording lock")? = id;
        Ok(())
    }
}
fn display(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = app.emit("mouse-stats://shown", ());
    }
}
#[tauri::command]
fn diagnostics(
    app: tauri::AppHandle,
    runtime: tauri::State<Arc<Runtime>>,
) -> Result<serde_json::Value, String> {
    let db = runtime.db.lock().map_err(|_| "DB lock")?;
    let integrity: String = db
        .conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok(
        serde_json::json!({"database_ok":integrity=="ok","input_ready":runtime.health.lock().map_err(|_|"Health lock")?.input_ready,"window_visible":app.get_webview_window("main").ok_or("Window missing")?.is_visible().map_err(|e|e.to_string())?,"autostart_enabled":app.autolaunch().is_enabled().map_err(|e|e.to_string())?}),
    )
}
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
#[tauri::command]
fn snapshot(runtime: tauri::State<Arc<Runtime>>) -> Result<Snapshot, String> {
    let db = runtime.db.lock().map_err(|_| "DB lock")?;
    let h = runtime.health.lock().map_err(|_| "Health lock")?;
    let signed_in = runtime
        .session
        .lock()
        .map_err(|_| "Session lock")?
        .is_some();
    Ok(Snapshot {
        device_id: runtime.device.clone(),
        recording_id: runtime
            .recording
            .lock()
            .map_err(|_| "Recording lock")?
            .clone(),
        settings: db.settings(),
        rows: db.rows().map_err(|e| e.to_string())?,
        devices: db
            .get("devices")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        last_saved: db.get("last_saved").and_then(|s| s.parse().ok()),
        last_sync: db.get("last_sync").and_then(|s| s.parse().ok()),
        sync_status: idle_sync_status(db.settings().sync_enabled, signed_in)
            .map(String::from)
            .unwrap_or_else(|| h.sync_status.clone()),
        error: h.error.clone().or_else(|| h.sync_error.clone()),
        generation_conflict: h.generation_conflict,
        signed_in,
    })
}
#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    runtime: tauri::State<Arc<Runtime>>,
    settings: Settings,
) -> Result<(), String> {
    if !matches!(settings.layout.as_str(), "US" | "JIS") || settings.timezone.len() > 100 {
        return Err("設定が不正です".into());
    }
    if !settings.cloud_url.is_empty() {
        sync::cloud_url(&settings.cloud_url)?;
    }
    let previous_autostart = app.autolaunch().is_enabled().map_err(|e| e.to_string())?;
    if settings.autostart {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|e| e.to_string())?;
    if let Err(error) = runtime.persist_settings(&settings) {
        let rollback = if previous_autostart {
            app.autolaunch().enable()
        } else {
            app.autolaunch().disable()
        };
        if rollback.is_err() {
            runtime.error("設定の保存失敗後、自動起動設定も復元できませんでした");
        }
        return Err(error);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(if settings.consent && !settings.paused {
            "Mouse Stats — 計測中"
        } else {
            "Mouse Stats — 停止中"
        }));
    }
    runtime.sync_requested.store(true, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
fn begin_login(runtime: tauri::State<Arc<Runtime>>) -> Result<(), String> {
    sync::begin(&runtime)
}
#[tauri::command]
fn manage_account(runtime: tauri::State<Arc<Runtime>>) -> Result<(), String> {
    let s = runtime.db.lock().map_err(|_| "DB lock")?.settings();
    sync::open_url(&format!("{}/?mode=manage", sync::cloud_url(&s.cloud_url)?))
}
#[tauri::command]
async fn recover_device(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, Arc<Runtime>>,
) -> Result<(), String> {
    #[cfg(windows)]
    let hwnd = app
        .get_webview_window("main")
        .ok_or("Window not found")?
        .hwnd()
        .map_err(|e| e.to_string())?
        .0 as isize;
    #[cfg(not(windows))]
    let hwnd = 0isize;
    let rt = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || sync::recover(&rt, hwnd))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn register_new_device(
    app: tauri::AppHandle,
    runtime: tauri::State<Arc<Runtime>>,
) -> Result<(), String> {
    let _guard = runtime
        .sync_guard
        .try_lock()
        .map_err(|_| "同期中です。完了後にもう一度操作してください")?;
    runtime.prepare_disconnect(Some(&uuid::Uuid::new_v4().to_string()))?;
    if let Err(e) = sync::finish_session_cleanup(&runtime) {
        // The committed marker makes startup retry without restoring the old identity.
        runtime.error(&e);
    }
    drop(_guard);
    app.restart();
}
#[tauri::command]
fn sync_now(runtime: tauri::State<Arc<Runtime>>) {
    runtime.sync_requested.store(true, Ordering::Relaxed);
}
#[tauri::command]
fn resolve_generation(runtime: tauri::State<Arc<Runtime>>, keep_local: bool) -> Result<(), String> {
    let _guard = runtime
        .sync_guard
        .try_lock()
        .map_err(|_| "同期中です。完了後にもう一度操作してください")?;
    let _writer = runtime.writer_guard.lock().map_err(|_| "Writer lock")?;
    runtime.flush_locked()?;
    let generation = runtime
        .session
        .lock()
        .map_err(|_| "Session lock")?
        .as_ref()
        .ok_or("ログインが必要です")?
        .generation;
    let id = runtime.db.lock().map_err(|_| "DB lock")?.reset_recording(
        &runtime.device,
        Some(generation),
        keep_local,
        false,
    )?;
    *runtime.recording.lock().map_err(|_| "Recording lock")? = id;
    runtime
        .health
        .lock()
        .map_err(|_| "Health lock")?
        .generation_conflict = false;
    runtime.sync_requested.store(true, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
fn delete_local(runtime: tauri::State<Arc<Runtime>>) -> Result<(), String> {
    let _guard = runtime
        .sync_guard
        .try_lock()
        .map_err(|_| "同期中です。完了後にもう一度操作してください")?;
    runtime.reset_local()
}
#[tauri::command]
fn logout(runtime: tauri::State<Arc<Runtime>>) -> Result<(), String> {
    let _guard = runtime
        .sync_guard
        .try_lock()
        .map_err(|_| "同期中です。完了後にもう一度操作してください")?;
    runtime.prepare_disconnect(None)?;
    sync::finish_session_cleanup(&runtime)
}
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| display(app)))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args(["--background"])
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            snapshot,
            diagnostics,
            quit_app,
            save_settings,
            begin_login,
            manage_account,
            recover_device,
            register_new_device,
            sync_now,
            resolve_generation,
            delete_local,
            logout
        ])
        .setup(|app| {
            let path = app.path().app_data_dir()?;
            std::fs::create_dir_all(&path)?;
            let db = db::Db::open(&path.join("statistics.db"))?;
            let device = db.device_id()?;
            let identity_path=path.join(format!("identity-{device}.dpapi"));
            let secret_path = path.join("session.dpapi");
            let pending_path = path.join("pending-login.dpapi");
            let pending = sync::restore_pending(&pending_path);
            let cleanup_pending=db.cleanup_mode()?.is_some();
            let session=sync::restore_session(&db,&secret_path)?;
            let mut health=Health::default();if secret_path.exists()&&session.is_none()&&!cleanup_pending{health.error=Some("保存したログイン情報を復号できません。再ログインするか、新しいPCとして再登録してください".into());}
            let recording = db.start(&device, session.is_some())?;
            let settings = db.settings();
            let autostart_result=if settings.autostart{app.autolaunch().enable()}else{app.autolaunch().disable()};
            if autostart_result.is_err(){health.error=Some("Windows自動起動の設定を反映できませんでした".into());}
            let runtime = Arc::new(Runtime {
                db: Mutex::new(db),
                aggregate: Mutex::new(aggregate::Aggregator::new(
                    !settings.consent || settings.paused,
                )),
                recording: Mutex::new(recording),
                device,
                health: Mutex::new(health),
                session: Mutex::new(session),
                pending: Mutex::new(pending),
                secret_path,
                pending_path,
                identity_path,
                writer_guard:Mutex::new(()),
                #[cfg(test)] after_drain:Mutex::new(None),
                sync_guard: Mutex::new(()),
                sync_requested: AtomicBool::new(true),
                stopping: AtomicBool::new(false),
            });
            app.manage(runtime.clone());
            let open = MenuItem::with_id(app, "open", "統計を開く", true, None::<&str>)?;
            let pause =
                MenuItem::with_id(app, "pause", "計測を一時停止 / 再開", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &pause, &quit])?;
            let icon = tauri::image::Image::new_owned([89, 208, 180, 255].repeat(32 * 32), 32, 32);
            TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&menu)
                .tooltip(if settings.consent && !settings.paused {
                    "Mouse Stats — 計測中"
                } else {
                    "Mouse Stats — 停止中"
                })
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        display(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| {
                    let runtime = app.state::<Arc<Runtime>>();
                    match event.id.as_ref() {
                        "open" => display(app),
                        "pause" => {
                            let mut s = runtime.db.lock().unwrap().settings();
                            s.paused = !s.paused;
                            if let Err(e) = save_settings(app.clone(), runtime, s) {
                                app.state::<Arc<Runtime>>().error(&e);
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .build(app)?;
            let rt = runtime.clone();
            std::thread::spawn(move || {
                while !rt.stopping.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(5));
                    let _ = rt.flush();
                }
            });
            let rt = runtime.clone();
            std::thread::spawn(move || {
                let mut next = Instant::now();
                let mut logout_attempt=Instant::now();
                let mut online=sync::network_available();
                let mut failures = 0u32;
                while !rt.stopping.load(Ordering::Relaxed) {
                    let connected=sync::network_available();if connected&&!online{rt.sync_requested.store(true,Ordering::Relaxed);}online=connected;
                    if Instant::now()>=logout_attempt{let _=sync::drain_logouts(&rt);logout_attempt=Instant::now()+Duration::from_secs(30);}
                    if rt.sync_requested.swap(false, Ordering::Relaxed) || Instant::now() >= next {
                        let enabled = rt
                            .db
                            .lock()
                            .map(|d| d.settings().sync_enabled)
                            .unwrap_or(false);
                        let signed_in = rt.session.lock().map(|s| s.is_some()).unwrap_or(false);
                        if let Some(status) = idle_sync_status(enabled, signed_in) {
                            failures = 0;
                            let mut h = rt.health.lock().unwrap();
                            h.sync_status = status.into();
                            h.sync_error = None;
                        } else if enabled {
                            rt.health.lock().unwrap().sync_status = "同期中".into();
                            match sync::sync(&rt) {
                                Ok(()) => {
                                    failures = 0;
                                    let mut h = rt.health.lock().unwrap();
                                    h.sync_status = "同期済み".into();
                                    h.sync_error = None;
                                }
                                Err(e) => {
                                    failures = (failures + 1).min(8);
                                    let mut h = rt.health.lock().unwrap();
                                    h.sync_status = if e == "NETWORK_UNAVAILABLE" {
                                        "オフライン".into()
                                    } else {
                                        "エラー".into()
                                    };
                                    h.sync_error = Some(e);
                                }
                            }
                        }
                        let delay = if failures == 0 {
                            300
                        } else {
                            (5u64 * 2u64.pow(failures)).min(300)
                                + u64::from(rand::random::<u8>()) % 10
                        };
                        next = Instant::now() + Duration::from_secs(delay);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
            #[cfg(windows)] match input::start(runtime.clone()).recv_timeout(Duration::from_secs(10)){Ok(Ok(_))=>runtime.health.lock().unwrap().input_ready=true,_=>runtime.error("入力計測を開始できませんでした")}
            #[cfg(not(windows))] input::start(runtime.clone());
            let rt = runtime.clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    sync::callback(rt.clone(), url.to_string());
                }
            });
            if let Some(urls) = app.deep_link().get_current()? {
                for url in urls {
                    sync::callback(runtime.clone(), url.to_string());
                }
            }
            if !std::env::args().any(|a| a == "--background") {
                display(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("Mouse Stats startup failed");
    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            let rt = app.state::<Arc<Runtime>>();
            rt.stopping.store(true, Ordering::Relaxed);
            let _ = rt.flush();
            if let (Ok(db), Ok(id)) = (rt.db.lock(), rt.recording.lock()) {
                let _ = db.conn.execute(
                    "UPDATE recordings SET ended_at=? WHERE id=?",
                    rusqlite::params![now(), *id],
                );
            };
        }
    });
}
