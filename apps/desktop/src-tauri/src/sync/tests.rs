use super::*;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
    time::Duration,
};
use tiny_http::{Header, Response, Server};

struct Fixture {
    runtime: Arc<Runtime>,
    paths: Arc<std::sync::Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    directory: std::path::PathBuf,
}
impl Fixture {
    fn new(
        mut handler: impl FnMut(&Arc<Runtime>, &str, Value) -> (u16, Value) + Send + 'static,
    ) -> Self {
        let server = Server::http(("127.0.0.1", 0)).unwrap();
        let base = format!("http://{}", server.server_addr());
        let directory =
            std::env::temp_dir().join(format!("mouse-stats-sync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let mut runtime = crate::native_tests::memory_runtime(false);
        Arc::get_mut(&mut runtime).unwrap().secret_path = directory.join("session.dpapi");
        Arc::get_mut(&mut runtime).unwrap().pending_path = directory.join("pending-login.dpapi");
        Arc::get_mut(&mut runtime).unwrap().identity_path = directory.join("identity.dpapi");
        {
            let mut db = runtime.db.lock().unwrap();
            db.save_settings(&Settings {
                consent: true,
                sync_enabled: true,
                cloud_url: base.clone(),
                ..Default::default()
            })
            .unwrap();
            db.conn
                .execute("UPDATE recordings SET eligible=1", [])
                .unwrap();
            let id = runtime.recording.lock().unwrap().clone();
            db.flush(&[
                Row {
                    recording_id: id.clone(),
                    hour_start: 0,
                    key_code: None,
                    value: 5000,
                    revision: 0,
                },
                Row {
                    recording_id: id,
                    hour_start: 0,
                    key_code: Some(30),
                    value: 2,
                    revision: 0,
                },
            ])
            .unwrap();
            db.cache(&[remote_row(9)]).unwrap();
            db.set("last_sync", "100").unwrap();
        }
        *runtime.session.lock().unwrap() = Some(Session {
            access_token: "o".repeat(43),
            refresh_token: "r".repeat(43),
            user_id: "00000000-0000-4000-8000-000000000004".into(),
            generation: 0,
            cloud_url: base,
            refresh_nonce: None,
        });
        let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let rt = runtime.clone();
        let seen = paths.clone();
        let done = stop.clone();
        let worker = std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                let Some(mut request) = server.recv_timeout(Duration::from_millis(50)).unwrap()
                else {
                    continue;
                };
                let path = request.url().to_string();
                seen.lock().unwrap().push(path.clone());
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let body = if body.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&body).unwrap()
                };
                let authenticated = path == "/api/v1/auth/refresh"
                    || path == "/api/v1/auth/device/logout"
                    || path == "/api/v1/auth/device/exchange"
                    || request.headers().iter().any(|h| {
                        h.field.equiv("Authorization")
                            && h.value.as_str() == format!("Bearer {}", "a".repeat(43))
                    });
                let (status, value) = if authenticated {
                    handler(&rt, &path, body)
                } else {
                    (401, json!({"error":"AUTHENTICATION_REQUIRED"}))
                };
                request
                    .respond(
                        Response::from_string(value.to_string())
                            .with_status_code(status)
                            .with_header(
                                Header::from_bytes(
                                    b"Content-Type".as_slice(),
                                    b"application/json".as_slice(),
                                )
                                .unwrap(),
                            ),
                    )
                    .unwrap();
            }
        });
        Self {
            runtime,
            paths,
            stop,
            worker: Some(worker),
            directory,
        }
    }
    fn paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        for file in [
            "session.dpapi",
            "session.tmp",
            "logout.dpapi",
            "logout.tmp",
            "identity.dpapi",
            "identity.tmp",
        ] {
            let _ = std::fs::remove_file(self.directory.join(file));
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}
fn refresh() -> Value {
    json!({"access_token":"a".repeat(43),"refresh_token":"n".repeat(43),"generation":0})
}
fn ack(body: &Value) -> Value {
    json!({"generation":0,"acked":body["rows"].as_array().unwrap().iter().map(|r|json!({"recording_id":r["recording_id"],"hour_start":r["hour_start"],"key_code":r["key_code"],"revision":r["revision"]})).collect::<Vec<_>>()})
}
fn remote_row(value: i64) -> StatsRow {
    StatsRow {
        device_id: "00000000-0000-4000-8000-000000000002".into(),
        recording_id: "00000000-0000-4000-8000-000000000003".into(),
        hour_start: 0,
        key_code: Some(30),
        value,
    }
}
fn empty_pull() -> Value {
    json!({"generation":0,"rows":[],"recordings":[],"next_cursor":null})
}
fn download_reply(path: &str, body: &Value) -> (u16, Value) {
    if path == "/api/v1/auth/refresh" {
        (200, refresh())
    } else if path == "/api/v1/sync/push" {
        (200, ack(body))
    } else if path.starts_with("/api/v1/sync/pull") {
        (200, empty_pull())
    } else if path.starts_with("/api/v1/stats") {
        (
            200,
            json!({"generation":0,"rows":[remote_row(7)],"next_cursor":null}),
        )
    } else if path == "/api/v1/devices" {
        (200, json!({"devices":[]}))
    } else {
        (500, json!({}))
    }
}
fn disable(rt: &Runtime) {
    let db = rt.db.lock().unwrap();
    let mut settings = db.settings();
    settings.sync_enabled = false;
    db.save_settings(&settings).unwrap();
}
fn cached_value(rt: &Runtime) -> i64 {
    rt.db
        .lock()
        .unwrap()
        .rows()
        .unwrap()
        .iter()
        .filter(|r| r.device_id != rt.device)
        .map(|r| r.value)
        .sum()
}

#[test]
fn disabling_sync_during_refresh_prevents_upload_and_download() {
    let fixture = Fixture::new(|rt, path, _| {
        if path == "/api/v1/auth/refresh" {
            disable(rt);
            (200, refresh())
        } else {
            (500, json!({}))
        }
    });
    sync(&fixture.runtime).unwrap();
    assert_eq!(fixture.paths(), ["/api/v1/auth/refresh"]);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .pending()
            .unwrap()
            .1
            .len(),
        2
    );
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}
#[test]
fn disabling_sync_during_restore_stops_pagination() {
    let fixture = Fixture::new(|rt, path, body| {
        if path == "/api/v1/auth/refresh" {
            (200, refresh())
        } else if path == "/api/v1/sync/push" {
            (200, ack(&body))
        } else if path == "/api/v1/sync/pull?cursor=" {
            disable(rt);
            (
                200,
                json!({"generation":0,"rows":[],"recordings":[],"next_cursor":"next_page"}),
            )
        } else {
            (500, json!({}))
        }
    });
    sync(&fixture.runtime).unwrap();
    assert_eq!(fixture.paths().len(), 3);
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}
#[test]
fn disabling_sync_during_remote_download_preserves_the_previous_cache() {
    let fixture = Fixture::new(|rt, path, body| {
        if path == "/api/v1/auth/refresh" {
            (200, refresh())
        } else if path == "/api/v1/sync/push" {
            (200, ack(&body))
        } else if path.starts_with("/api/v1/sync/pull") {
            (200, empty_pull())
        } else if path.starts_with("/api/v1/stats") {
            disable(rt);
            (
                200,
                json!({"generation":0,"rows":[remote_row(100)],"next_cursor":"next_page"}),
            )
        } else {
            (500, json!({}))
        }
    });
    sync(&fixture.runtime).unwrap();
    assert_eq!(fixture.paths().len(), 4);
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}
#[test]
fn shutdown_after_upload_stops_before_restoring_remote_data() {
    let fixture = Fixture::new(|rt, path, body| {
        if path == "/api/v1/auth/refresh" {
            (200, refresh())
        } else if path == "/api/v1/sync/push" {
            rt.stopping.store(true, Ordering::Relaxed);
            (200, ack(&body))
        } else {
            (500, json!({}))
        }
    });
    sync(&fixture.runtime).unwrap();
    assert_eq!(fixture.paths().len(), 2);
}
#[test]
fn malformed_mouse_ack_cannot_mark_a_row_as_synced() {
    let fixture = Fixture::new(|_, path, body| {
        if path == "/api/v1/auth/refresh" {
            (200, refresh())
        } else if path == "/api/v1/sync/push" {
            let mut a = ack(&body);
            for row in a["acked"].as_array_mut().unwrap() {
                if row["key_code"].is_null() {
                    row["key_code"] = json!(-1);
                }
            }
            (200, a)
        } else {
            (500, json!({}))
        }
    });
    assert_eq!(sync(&fixture.runtime).unwrap_err(), "INVALID_ACK");
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .pending()
            .unwrap()
            .1
            .len(),
        2
    );
}
#[test]
fn failure_on_a_later_remote_page_keeps_the_last_complete_cache() {
    let fixture = Fixture::new(|_, path, body| {
        if path == "/api/v1/auth/refresh" {
            (200, refresh())
        } else if path == "/api/v1/sync/push" {
            (200, ack(&body))
        } else if path.starts_with("/api/v1/sync/pull") {
            (200, empty_pull())
        } else if path.starts_with("/api/v1/stats") && !path.ends_with("cursor=next_page") {
            (
                200,
                json!({"generation":0,"rows":[remote_row(100)],"next_cursor":"next_page"}),
            )
        } else {
            (503, json!({}))
        }
    });
    assert_eq!(sync(&fixture.runtime).unwrap_err(), "HTTP_503");
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}
#[test]
fn input_while_upload_is_in_flight_is_sent_with_a_later_revision() {
    let mut latest: Vec<Row> = Vec::new();
    let mut added = false;
    let fixture = Fixture::new(move |rt, path, body| {
        if path == "/api/v1/auth/refresh" {
            return (200, refresh());
        }
        if path == "/api/v1/sync/push" {
            let rows: Vec<Row> = serde_json::from_value(body["rows"].clone()).unwrap();
            for row in rows {
                if let Some(old) = latest.iter_mut().find(|r| {
                    r.recording_id == row.recording_id
                        && r.hour_start == row.hour_start
                        && r.key_code == row.key_code
                }) {
                    old.value = old.value.max(row.value);
                } else {
                    latest.push(row);
                }
            }
            if !added {
                added = true;
                rt.aggregate.lock().unwrap().mouse(0, 2, false, 1234);
                rt.flush().unwrap();
            }
            return (200, ack(&body));
        }
        if path.starts_with("/api/v1/sync/pull") {
            let id = rt.recording.lock().unwrap().clone();
            let started_at: i64 = rt
                .db
                .lock()
                .unwrap()
                .conn
                .query_row("SELECT started_at FROM recordings WHERE id=?", [&id], |r| {
                    r.get(0)
                })
                .unwrap();
            let rows: Vec<_> = latest
                .iter()
                .map(|r| StatsRow {
                    device_id: rt.device.clone(),
                    recording_id: r.recording_id.clone(),
                    hour_start: r.hour_start,
                    key_code: r.key_code,
                    value: r.value,
                })
                .collect();
            return (
                200,
                json!({"generation":0,"rows":rows,"recordings":[{"id":id,"started_at":started_at,"ended_at":null,"key_schema_version":1,"data_generation":0}],"next_cursor":null}),
            );
        }
        if path.starts_with("/api/v1/stats") {
            return (
                200,
                json!({"generation":0,"rows":[remote_row(7)],"next_cursor":null}),
            );
        }
        if path == "/api/v1/devices" {
            return (200, json!({"devices":[]}));
        }
        (500, json!({}))
    });
    sync(&fixture.runtime).unwrap();
    assert_eq!(
        fixture
            .paths()
            .iter()
            .filter(|p| p.as_str() == "/api/v1/sync/push")
            .count(),
        2
    );
    let db = fixture.runtime.db.lock().unwrap();
    assert!(db.pending().unwrap().1.is_empty());
    let rows = db.rows().unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r.device_id == fixture.runtime.device && r.key_code.is_none())
            .unwrap()
            .value,
        7000
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.key_code == Some(30))
            .map(|r| r.value)
            .sum::<i64>(),
        9
    );
    assert_ne!(db.get("last_sync").as_deref(), Some("100"));
}
#[test]
fn a_lost_refresh_response_reuses_the_nonce_after_reloading_dpapi() {
    let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = attempts.clone();
    let fixture = Fixture::new(move |_, path, body| {
        if path == "/api/v1/auth/refresh" {
            let nonce = body["nonce"].as_str().unwrap().to_string();
            let mut attempts = observed.lock().unwrap();
            attempts.push(nonce);
            return if attempts.len() == 1 {
                (503, json!({}))
            } else if attempts[0] == attempts[1] {
                (200, refresh())
            } else {
                (401, json!({}))
            };
        }
        if path == "/api/v1/sync/push" {
            return (200, ack(&body));
        }
        if path.starts_with("/api/v1/sync/pull") {
            return (200, empty_pull());
        }
        if path.starts_with("/api/v1/stats") {
            return (
                200,
                json!({"generation":0,"rows":[remote_row(7)],"next_cursor":null}),
            );
        }
        if path == "/api/v1/devices" {
            return (200, json!({"devices":[]}));
        }
        (500, json!({}))
    });
    assert_eq!(sync(&fixture.runtime).unwrap_err(), "HTTP_503");
    let saved: Session =
        serde_json::from_slice(&secret::load(&fixture.runtime.secret_path).unwrap()).unwrap();
    assert!(saved.refresh_nonce.is_some());
    *fixture.runtime.session.lock().unwrap() = Some(saved);
    sync(&fixture.runtime).unwrap();
    assert_eq!(attempts.lock().unwrap().len(), 2);
    let saved: Session =
        serde_json::from_slice(&secret::load(&fixture.runtime.secret_path).unwrap()).unwrap();
    assert!(saved.refresh_nonce.is_none());
    assert!(saved.refresh_token == "n".repeat(43));
    assert!(fixture
        .runtime
        .db
        .lock()
        .unwrap()
        .pending()
        .unwrap()
        .1
        .is_empty());
}
#[test]
fn malformed_device_metadata_does_not_replace_a_complete_cache() {
    let fixture = Fixture::new(|_, path, body| {
        if path == "/api/v1/devices" {
            (200, json!({"devices":"invalid"}))
        } else {
            download_reply(path, &body)
        }
    });
    assert_eq!(sync(&fixture.runtime).unwrap_err(), "INVALID_DEVICES");
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}
#[test]
fn failed_metadata_write_rolls_back_cache_and_last_success_time() {
    let fixture = Fixture::new(|_, path, body| download_reply(path, &body));
    fixture.runtime.db.lock().unwrap().conn.execute_batch("CREATE TRIGGER reject_devices BEFORE INSERT ON settings WHEN NEW.key='devices' BEGIN SELECT RAISE(ABORT,'test metadata failure'); END;").unwrap();
    assert!(sync(&fixture.runtime)
        .unwrap_err()
        .contains("test metadata failure"));
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}
#[test]
fn disabling_sync_during_device_listing_does_not_commit_downloads() {
    let fixture = Fixture::new(|rt, path, body| {
        if path == "/api/v1/devices" {
            disable(rt);
        }
        download_reply(path, &body)
    });
    sync(&fixture.runtime).unwrap();
    assert_eq!(fixture.paths().len(), 5);
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert_eq!(
        fixture
            .runtime
            .db
            .lock()
            .unwrap()
            .get("last_sync")
            .as_deref(),
        Some("100")
    );
}

fn persist_fixture_session(fixture: &Fixture) {
    fixture.runtime.health.lock().unwrap().generation_conflict = true;
    let session = fixture.runtime.session.lock().unwrap().clone().unwrap();
    secret::save(
        &fixture.runtime.secret_path,
        &serde_json::to_vec(&session).unwrap(),
    )
    .unwrap();
    fixture
        .runtime
        .db
        .lock()
        .unwrap()
        .set("account_id", &session.user_id)
        .unwrap();
    *fixture.runtime.pending.lock().unwrap() = Some(Pending {
        state: "pending".into(),
        verifier: random(),
        created: now(),
        cloud_url: session.cloud_url,
    });
}
#[cfg(windows)]
#[test]
fn pending_device_login_is_restored_after_restart_and_expires_after_five_minutes() {
    let fixture = Fixture::new(|_, _, _| (500, json!({})));
    let pending = Pending {
        state: "login-state".into(),
        verifier: "pkce-verifier".into(),
        created: now(),
        cloud_url: "https://mouse-stats.example".into(),
    };
    secret::save(
        &fixture.runtime.pending_path,
        &serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    let restored = restore_pending(&fixture.runtime.pending_path).unwrap();
    assert_eq!(restored.state, pending.state);
    assert_eq!(restored.verifier, pending.verifier);

    let expired = Pending {
        created: now() - 301,
        ..pending
    };
    secret::save(
        &fixture.runtime.pending_path,
        &serde_json::to_vec(&expired).unwrap(),
    )
    .unwrap();
    assert!(restore_pending(&fixture.runtime.pending_path).is_none());
}
fn queued_logouts(fixture: &Fixture) -> Vec<Session> {
    serde_json::from_slice(&secret::load(&fixture.directory.join("logout.dpapi")).unwrap()).unwrap()
}
#[test]
fn failed_disconnection_preserves_credentials_and_all_database_state() {
    let fixture = Fixture::new(|_, _, _| (500, json!({})));
    persist_fixture_session(&fixture);
    {
        let db = fixture.runtime.db.lock().unwrap();
        let mut settings = db.settings();
        settings.upload_history = true;
        db.save_settings(&settings).unwrap();
        db.set("devices", "[]").unwrap();
        db.conn.execute_batch("CREATE TRIGGER reject_disconnect BEFORE INSERT ON settings WHEN NEW.key='session_cleanup' BEGIN SELECT RAISE(ABORT,'test disconnect failure'); END;").unwrap();
    }
    assert!(fixture
        .runtime
        .prepare_disconnect(Some(&uuid::Uuid::new_v4().to_string()))
        .unwrap_err()
        .contains("test disconnect failure"));
    let db = fixture.runtime.db.lock().unwrap();
    assert_eq!(db.device_id().unwrap(), fixture.runtime.device);
    assert!(db.settings().sync_enabled && db.settings().upload_history);
    assert!(db.get("account_id").is_some());
    assert_eq!(db.get("last_sync").as_deref(), Some("100"));
    assert_eq!(db.get("devices").as_deref(), Some("[]"));
    assert!(db.get("session_cleanup").is_none());
    assert_eq!(db.pending().unwrap().1.len(), 2);
    assert!(restore_session(&db, &fixture.runtime.secret_path)
        .unwrap()
        .is_some());
    drop(db);
    assert_eq!(cached_value(&fixture.runtime), 9);
    assert!(fixture.runtime.session.lock().unwrap().is_some());
    assert!(fixture.runtime.pending.lock().unwrap().is_some());
    assert!(fixture.runtime.health.lock().unwrap().generation_conflict);
    assert!(!fixture.directory.join("logout.dpapi").exists());
}
#[test]
fn interrupted_logout_stays_signed_out_and_retries_revocation_after_restart() {
    let mut attempts = 0;
    let fixture = Fixture::new(move |_, path, body| {
        assert_eq!(path, "/api/v1/auth/device/logout");
        assert!(body["refresh_token"] == "r".repeat(43));
        attempts += 1;
        (if attempts == 1 { 503 } else { 200 }, json!({}))
    });
    persist_fixture_session(&fixture);
    // A filesystem failure must leave a durable marker and an inactive credential.
    let outbox = fixture.directory.join("logout.dpapi");
    std::fs::create_dir(&outbox).unwrap();
    fixture.runtime.prepare_disconnect(None).unwrap();
    assert!(finish_session_cleanup(&fixture.runtime).is_err());
    assert!(fixture.runtime.secret_path.exists());
    assert!(fixture.runtime.session.lock().unwrap().is_none());
    assert!(fixture.runtime.pending.lock().unwrap().is_none());
    assert!(!fixture.runtime.health.lock().unwrap().generation_conflict);
    {
        let db = fixture.runtime.db.lock().unwrap();
        assert!(!db.settings().sync_enabled && !db.settings().upload_history);
        assert!(db.get("account_id").is_none());
        assert!(db.get("last_sync").is_none());
        assert_eq!(db.get("session_cleanup").as_deref(), Some("logout"));
        assert!(restore_session(&db, &fixture.runtime.secret_path)
            .unwrap()
            .is_none());
        assert!(db.pending().unwrap().1.is_empty());
        assert_eq!(db.rows().unwrap().len(), 2); // local statistics remain
    }
    std::fs::remove_dir(outbox).unwrap();
    finish_session_cleanup(&fixture.runtime).unwrap();
    assert!(!fixture.runtime.secret_path.exists());
    assert!(fixture
        .runtime
        .db
        .lock()
        .unwrap()
        .get("session_cleanup")
        .is_none());
    assert_eq!(queued_logouts(&fixture).len(), 1);
    drain_logouts(&fixture.runtime).unwrap(); // offline: durable queue retained
    assert_eq!(queued_logouts(&fixture).len(), 1);
    drain_logouts(&fixture.runtime).unwrap();
    assert!(!fixture.directory.join("logout.dpapi").exists());
    assert_eq!(fixture.paths().len(), 2);
}
#[test]
fn retry_after_credential_delete_failure_does_not_duplicate_the_revocation_queue() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new(|_, _, _| (500, json!({})));
    persist_fixture_session(&fixture);
    // Allow reading, but deny deletion as another Windows handle can do.
    let open = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&fixture.runtime.secret_path)
        .unwrap();
    fixture.runtime.prepare_disconnect(None).unwrap();
    assert!(finish_session_cleanup(&fixture.runtime).is_err());
    assert_eq!(queued_logouts(&fixture).len(), 1);
    drop(open);
    finish_session_cleanup(&fixture.runtime).unwrap();
    finish_session_cleanup(&fixture.runtime).unwrap();
    assert_eq!(queued_logouts(&fixture).len(), 1);
    assert!(!fixture.runtime.secret_path.exists());
}
#[test]
fn reregistration_preserves_statistics_without_revoking_the_cloned_device() {
    let fixture = Fixture::new(|_, _, _| (500, json!({})));
    persist_fixture_session(&fixture);
    let replacement = uuid::Uuid::new_v4().to_string();
    fixture
        .runtime
        .prepare_disconnect(Some(&replacement))
        .unwrap();
    {
        let db = fixture.runtime.db.lock().unwrap();
        assert_eq!(db.device_id().unwrap(), replacement);
        assert!(db.get("account_id").is_none());
        assert_eq!(db.get("session_cleanup").as_deref(), Some("replace_device"));
        assert!(restore_session(&db, &fixture.runtime.secret_path)
            .unwrap()
            .is_none());
        assert!(db.pending().unwrap().1.is_empty());
        assert_eq!(db.rows().unwrap().len(), 2);
        assert!(db
            .rows()
            .unwrap()
            .iter()
            .all(|r| r.device_id == fixture.runtime.device));
    }
    finish_session_cleanup(&fixture.runtime).unwrap();
    assert!(!fixture.runtime.secret_path.exists());
    assert!(!fixture.directory.join("logout.dpapi").exists());
    assert!(fixture.paths().is_empty());
}
#[test]
fn opting_in_to_history_after_reregistration_does_not_reassign_another_device() {
    let fixture = Fixture::new(|_, path, _| {
        assert_eq!(path, "/api/v1/auth/device/exchange");
        (
            200,
            json!({"access_token":"a".repeat(43),"refresh_token":"n".repeat(43),"user_id":"00000000-0000-4000-8000-000000000004","generation":0}),
        )
    });
    let older;
    {
        let mut db = fixture.runtime.db.lock().unwrap();
        let mut settings = db.settings();
        settings.upload_history = true;
        db.save_settings(&settings).unwrap();
        older = db.start(&uuid::Uuid::new_v4().to_string(), false).unwrap();
        db.flush(&[Row {
            recording_id: older.clone(),
            hour_start: 0,
            key_code: Some(31),
            value: 7,
            revision: 0,
        }])
        .unwrap();
        db.conn
            .execute("UPDATE recordings SET eligible=0", [])
            .unwrap();
    }
    *fixture.runtime.pending.lock().unwrap() = Some(Pending {
        state: "pending".into(),
        verifier: random(),
        created: now(),
        cloud_url: fixture
            .runtime
            .session
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .cloud_url
            .clone(),
    });
    exchange(
        &fixture.runtime,
        "mousestats://auth?state=pending&code=single-use",
    )
    .unwrap();
    let db = fixture.runtime.db.lock().unwrap();
    let (records, rows) = db.pending().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(records.iter().all(|r| r.id != older));
    assert!(db
        .rows()
        .unwrap()
        .iter()
        .any(|r| r.recording_id == older && r.value == 7));
}
#[test]
fn reopening_the_database_after_logout_cannot_restore_the_saved_session() {
    let fixture = Fixture::new(|_, _, _| (500, json!({})));
    persist_fixture_session(&fixture);
    let path = fixture.directory.join("restart.db");
    {
        let mut db = crate::db::Db::open(&path).unwrap();
        assert!(restore_session(&db, &fixture.runtime.secret_path)
            .unwrap()
            .is_some());
        db.disconnect(None).unwrap();
    }
    {
        let db = crate::db::Db::open(&path).unwrap();
        assert_eq!(db.cleanup_mode().unwrap().as_deref(), Some("logout"));
        assert!(restore_session(&db, &fixture.runtime.secret_path)
            .unwrap()
            .is_none());
    }
    std::fs::remove_file(path).unwrap();
}
