use crate::{model::*, secret, Runtime};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub user_id: String,
    pub generation: i64,
    pub cloud_url: String,
    #[serde(default)]
    pub refresh_nonce: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub state: String,
    pub verifier: String,
    pub created: i64,
    pub cloud_url: String,
}
pub fn restore_session(
    db: &crate::db::Db,
    path: &std::path::Path,
) -> rusqlite::Result<Option<Session>> {
    if db.cleanup_mode()?.is_some() {
        return Ok(None);
    }
    Ok(secret::load(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
}
pub fn restore_pending(path: &std::path::Path) -> Option<Pending> {
    let pending: Pending = secret::load(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    let age = now().saturating_sub(pending.created);
    (age <= 300 && age >= 0).then_some(pending)
}
pub fn random() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
pub fn network_available() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Networking::WinInet::{InternetGetConnectedState, INTERNET_CONNECTION};
        InternetGetConnectedState(&mut INTERNET_CONNECTION::default(), None).is_ok()
    }
    #[cfg(not(windows))]
    {
        true
    }
}
pub fn cloud_url(raw: &str) -> Result<String, String> {
    let url = url::Url::parse(raw).map_err(|_| "クラウドURLが不正です")?;
    let local = cfg!(debug_assertions)
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if !(url.scheme() == "https" || local)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("HTTPS のオリジンURLを指定してください".into());
    }
    Ok(url.origin().ascii_serialization())
}
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::{
            core::PCWSTR,
            Win32::{
                Foundation::HWND,
                UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
            },
        };
        let wide: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
        let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let result = ShellExecuteW(
            Some(HWND::default()),
            PCWSTR(verb.as_ptr()),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            return Err("ブラウザを開けませんでした".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("Windows required".into())
    }
}
fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}
fn response(res: reqwest::blocking::Response) -> Result<Value, String> {
    let status = res.status();
    // Never include response bodies, tokens or URLs in diagnostic errors.
    if !status.is_success() {
        return Err(format!("HTTP_{}", status.as_u16()));
    }
    res.json().map_err(|_| "クラウド応答が不正です".into())
}
pub fn begin(runtime: &Runtime) -> Result<(), String> {
    if runtime
        .db
        .lock()
        .map_err(|_| "DB lock")?
        .cleanup_mode()
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(
            "前のログイン情報を削除中です。しばらく待つか、新しいPCとして再登録してください".into(),
        );
    }
    let settings = runtime.db.lock().map_err(|_| "DB lock")?.settings();
    let base = cloud_url(&settings.cloud_url)?;
    let p = Pending {
        state: random(),
        verifier: random(),
        created: now(),
        cloud_url: base.clone(),
    };
    secret::save(
        &runtime.pending_path,
        &serde_json::to_vec(&p).map_err(|e| e.to_string())?,
    )?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
    let url = format!("{base}/?state={}&pkce={challenge}&mode=device", p.state);
    *runtime.pending.lock().map_err(|_| "Auth lock")? = Some(p);
    if let Err(error) = open_url(&url) {
        *runtime.pending.lock().map_err(|_| "Auth lock")? = None;
        let _ = std::fs::remove_file(&runtime.pending_path);
        return Err(error);
    }
    Ok(())
}
pub fn callback(runtime: Arc<Runtime>, url: String) {
    std::thread::spawn(move || {
        if let Err(e) = exchange(&runtime, &url) {
            runtime.error(&e);
        }
    });
}
fn exchange(runtime: &Runtime, raw: &str) -> Result<(), String> {
    let _guard = runtime.sync_guard.lock().map_err(|_| "Sync lock")?;
    if runtime
        .db
        .lock()
        .map_err(|_| "DB lock")?
        .cleanup_mode()
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("前のログイン情報の削除が完了していません".into());
    }
    let u = url::Url::parse(raw).map_err(|_| "認証URLが不正です")?;
    if u.scheme() != "mousestats" || u.host_str() != Some("auth") {
        return Err("認証URLが不正です".into());
    }
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    let mut pending = runtime.pending.lock().map_err(|_| "Auth lock")?;
    if pending.is_none() {
        *pending = restore_pending(&runtime.pending_path);
    }
    let p = pending.as_ref().ok_or("ログイン要求がありません")?;
    if q.get("state") != Some(&p.state) || now() - p.created > 300 {
        return Err("ログイン要求が期限切れか一致しません".into());
    }
    let p = pending.take().ok_or("ログイン要求がありません")?;
    drop(pending);
    let code = q.get("code").ok_or("認証コードがありません")?;
    let v=response(client()?.post(format!("{}/api/v1/auth/device/exchange",p.cloud_url)).json(&json!({"code":code,"state":p.state,"verifier":p.verifier,"device_id":runtime.device,"recovery_key":crate::identity::public(&runtime.identity_path)?,"name":std::env::var("COMPUTERNAME").unwrap_or_else(|_|"Windows PC".into())})).send().map_err(|_|"NETWORK_UNAVAILABLE")?)?;
    let session = Session {
        access_token: required(&v, "access_token")?,
        refresh_token: required(&v, "refresh_token")?,
        user_id: required(&v, "user_id")?,
        generation: v["generation"].as_i64().ok_or("INVALID_GENERATION")?,
        cloud_url: p.cloud_url,
        refresh_nonce: None,
    };
    let _writer = runtime.writer_guard.lock().map_err(|_| "Writer lock")?;
    runtime.flush_locked()?;
    let db = runtime.db.lock().map_err(|_| "DB lock")?;
    if db.get("account_id").is_some_and(|id| id != session.user_id) {
        return Err("別アカウントへ接続するには設定からログアウトしてください".into());
    }
    let first = db.get("account_id").is_none();
    if !first && db.generation() != session.generation {
        runtime
            .health
            .lock()
            .map_err(|_| "Health lock")?
            .generation_conflict = true;
    } else {
        db.set("generation", &session.generation.to_string())
            .map_err(|e| e.to_string())?;
        if first && db.settings().upload_history {
            db.conn
                .execute(
                    "UPDATE recordings SET eligible=1,data_generation=? WHERE device_id=?",
                    rusqlite::params![session.generation, runtime.device],
                )
                .map_err(|e| e.to_string())?;
        }
    }
    db.set("account_id", &session.user_id)
        .map_err(|e| e.to_string())?;
    let id = db.start(&runtime.device, true).map_err(|e| e.to_string())?;
    // Flush old recording before switching its ID.
    drop(db);
    *runtime.recording.lock().map_err(|_| "Recording lock")? = id;
    secret::save(
        &runtime.secret_path,
        &serde_json::to_vec(&session).map_err(|e| e.to_string())?,
    )?;
    let _ = std::fs::remove_file(&runtime.pending_path);
    *runtime.session.lock().map_err(|_| "Session lock")? = Some(session);
    runtime
        .sync_requested
        .store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}
fn required(v: &Value, k: &str) -> Result<String, String> {
    v[k].as_str()
        .map(String::from)
        .ok_or_else(|| format!("INVALID_{k}"))
}
fn save(runtime: &Runtime, session: &Session) -> Result<(), String> {
    secret::save(
        &runtime.secret_path,
        &serde_json::to_vec(session).map_err(|e| e.to_string())?,
    )?;
    *runtime.session.lock().map_err(|_| "Session lock")? = Some(session.clone());
    Ok(())
}
fn queue_logout(runtime: &Runtime, session: Session) -> Result<(), String> {
    let path = runtime.secret_path.with_file_name("logout.dpapi");
    let mut outbox: Vec<Session> = if path.exists() {
        serde_json::from_slice(&secret::load(&path)?)
            .map_err(|_| "ログアウト待機データが不正です")?
    } else {
        Vec::new()
    };
    if !outbox
        .iter()
        .any(|s| s.refresh_token == session.refresh_token && s.cloud_url == session.cloud_url)
    {
        outbox.push(session);
    }
    secret::save(
        &path,
        &serde_json::to_vec(&outbox).map_err(|e| e.to_string())?,
    )
}
// Caller holds sync_guard. A DB marker keeps the saved credential inactive across crashes.
pub fn finish_session_cleanup(runtime: &Runtime) -> Result<(), String> {
    let Some(mode) = runtime
        .db
        .lock()
        .map_err(|_| "DB lock")?
        .cleanup_mode()
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    if runtime.secret_path.exists() {
        if mode == "logout" {
            let session = serde_json::from_slice(&secret::load(&runtime.secret_path)?)
                .map_err(|_| "ログアウト待機データが不正です")?;
            queue_logout(runtime, session)?;
        }
        std::fs::remove_file(&runtime.secret_path).map_err(|e| e.to_string())?;
    }
    runtime
        .db
        .lock()
        .map_err(|_| "DB lock")?
        .conn
        .execute("DELETE FROM settings WHERE key='session_cleanup'", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn drain_logouts(runtime: &Runtime) -> Result<(), String> {
    let _guard = runtime.sync_guard.lock().map_err(|_| "Sync lock")?;
    finish_session_cleanup(runtime)?;
    let path = runtime.secret_path.with_file_name("logout.dpapi");
    if !path.exists() {
        return Ok(());
    }
    let outbox: Vec<Session> = serde_json::from_slice(&secret::load(&path)?)
        .map_err(|_| "ログアウト待機データが不正です")?;
    let client = client()?;
    let mut pending = Vec::new();
    for session in outbox {
        let result = client
            .post(format!("{}/api/v1/auth/device/logout", session.cloud_url))
            .json(&json!({"refresh_token":session.refresh_token}))
            .send();
        if !result.is_ok_and(|r| r.status().is_success()) {
            pending.push(session);
        }
    }
    if pending.is_empty() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    } else {
        secret::save(
            &path,
            &serde_json::to_vec(&pending).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(())
}
fn request(
    client: &reqwest::blocking::Client,
    session: &Session,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{}{path}", session.cloud_url);
    let builder = if let Some(b) = body {
        client.post(url).json(&b)
    } else {
        client.get(url)
    };
    response(
        builder
            .bearer_auth(&session.access_token)
            .send()
            .map_err(|_| "NETWORK_UNAVAILABLE")?,
    )
}
fn refresh_session(
    runtime: &Runtime,
    client: &reqwest::blocking::Client,
    session: &mut Session,
) -> Result<i64, String> {
    if session.refresh_nonce.is_none() {
        session.refresh_nonce = Some(random());
        save(runtime, session)?;
    }
    let refresh = response(
        client
            .post(format!("{}/api/v1/auth/refresh", session.cloud_url))
            .json(&json!({"refresh_token":session.refresh_token,"nonce":session.refresh_nonce}))
            .send()
            .map_err(|_| "NETWORK_UNAVAILABLE")?,
    )?;
    session.access_token = required(&refresh, "access_token")?;
    session.refresh_token = required(&refresh, "refresh_token")?;
    session.refresh_nonce = None;
    let generation = refresh["generation"].as_i64().ok_or("INVALID_GENERATION")?;
    session.generation = generation;
    save(runtime, session)?;
    Ok(generation)
}
pub fn recover(runtime: &Runtime, window: isize) -> Result<(), String> {
    let _guard = runtime.sync_guard.lock().map_err(|_| "Sync lock")?;
    let mut session = runtime
        .session
        .lock()
        .map_err(|_| "Session lock")?
        .clone()
        .ok_or("ログイン済みの端末が必要です")?;
    let client = client()?;
    refresh_session(runtime, &client, &mut session)?;
    let options = request(
        &client,
        &session,
        "/api/v1/device-recovery/options",
        Some(json!({})),
    )?;
    crate::identity::verify_user(window)?;
    let signature =
        crate::identity::sign(&runtime.identity_path, &required(&options, "challenge")?)?;
    let verified = request(
        &client,
        &session,
        "/api/v1/device-recovery/verify",
        Some(json!({"challenge_id":required(&options,"challenge_id")?,"signature":signature})),
    )?;
    open_url(&format!(
        "{}/?mode=manage&device_recovery={}",
        session.cloud_url,
        required(&verified, "code")?
    ))
}
pub fn sync(runtime: &Runtime) -> Result<(), String> {
    let _guard = runtime.sync_guard.lock().map_err(|_| "Sync lock")?;
    let mut session = runtime
        .session
        .lock()
        .map_err(|_| "Session lock")?
        .clone()
        .ok_or("ログインが必要です")?;
    let settings = runtime.db.lock().map_err(|_| "DB lock")?.settings();
    if !settings.sync_enabled || runtime.stopping.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    if runtime
        .health
        .lock()
        .map_err(|_| "Health lock")?
        .generation_conflict
    {
        return Err("GENERATION_CHANGED".into());
    }
    if cloud_url(&settings.cloud_url)? != session.cloud_url {
        return Err("ログイン先とクラウドURLが異なります".into());
    }
    let client = client()?;
    let generation = refresh_session(runtime, &client, &mut session)?;
    if !sync_allowed(runtime)? {
        return Ok(());
    }
    if generation != runtime.db.lock().map_err(|_| "DB lock")?.generation() {
        runtime
            .health
            .lock()
            .map_err(|_| "Health lock")?
            .generation_conflict = true;
        session.generation = generation;
        save(runtime, &session)?;
        return Err("GENERATION_CHANGED".into());
    }
    runtime.flush()?;
    loop {
        if !sync_allowed(runtime)? {
            return Ok(());
        }
        let (recordings, rows) = runtime
            .db
            .lock()
            .map_err(|_| "DB lock")?
            .pending()
            .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            break;
        }
        let ack = request(
            &client,
            &session,
            "/api/v1/sync/push",
            Some(json!({"generation":generation,"recordings":recordings,"rows":rows})),
        )?;
        let acked = ack["acked"].as_array().ok_or("INVALID_ACK")?;
        if ack["generation"].as_i64() != Some(generation)
            || acked.len() != rows.len()
            || !rows.iter().zip(acked).all(|(r, a)| {
                a["recording_id"].as_str() == Some(&r.recording_id)
                    && a["hour_start"].as_i64() == Some(r.hour_start)
                    && a["revision"].as_i64() == Some(r.revision)
                    && a.get("key_code") == Some(&json!(r.key_code))
            })
        {
            return Err("INVALID_ACK".into());
        }
        runtime
            .db
            .lock()
            .map_err(|_| "DB lock")?
            .ack(&rows)
            .map_err(|e| e.to_string())?;
    }
    let mut cursor = String::new();
    loop {
        if !sync_allowed(runtime)? {
            return Ok(());
        }
        let v = request(
            &client,
            &session,
            &format!("/api/v1/sync/pull?cursor={cursor}"),
            None,
        )?;
        if v["generation"].as_i64() != Some(generation) {
            return Err("GENERATION_CHANGED".into());
        }
        if !sync_allowed(runtime)? {
            return Ok(());
        }
        let records: Vec<Recording> =
            serde_json::from_value(v["recordings"].clone()).map_err(|_| "INVALID_RECORDINGS")?;
        let rows: Vec<StatsRow> =
            serde_json::from_value(v["rows"].clone()).map_err(|_| "INVALID_ROWS")?;
        runtime
            .db
            .lock()
            .map_err(|_| "DB lock")?
            .restore(&records, &rows, &runtime.device)
            .map_err(|e| e.to_string())?;
        match v["next_cursor"].as_str() {
            Some(c) if c != cursor => cursor = c.to_string(),
            None => break,
            _ => return Err("INVALID_CURSOR".into()),
        }
    }
    cursor.clear();
    let mut cached = Vec::new();
    loop {
        if !sync_allowed(runtime)? {
            return Ok(());
        }
        let v = request(
            &client,
            &session,
            &format!(
                "/api/v1/stats?exclude_device={}&cursor={cursor}",
                runtime.device
            ),
            None,
        )?;
        if v["generation"].as_i64() != Some(generation) {
            return Err("GENERATION_CHANGED".into());
        }
        if !sync_allowed(runtime)? {
            return Ok(());
        }
        let rows: Vec<StatsRow> =
            serde_json::from_value(v["rows"].clone()).map_err(|_| "INVALID_ROWS")?;
        let db = runtime.db.lock().map_err(|_| "DB lock")?;
        cached.extend(rows.into_iter().filter(|r| {
            r.device_id != runtime.device
                && !db
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM recordings WHERE id=?)",
                        [&r.recording_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap_or(true)
        }));
        match v["next_cursor"].as_str() {
            Some(c) if c != cursor => cursor = c.to_string(),
            None => break,
            _ => return Err("INVALID_CURSOR".into()),
        }
    }
    if !sync_allowed(runtime)? {
        return Ok(());
    }
    let devices = request(&client, &session, "/api/v1/devices", None)?;
    let devices: Vec<Device> =
        serde_json::from_value(devices["devices"].clone()).map_err(|_| "INVALID_DEVICES")?;
    let mut db = runtime.db.lock().map_err(|_| "DB lock")?;
    if !db.settings().sync_enabled || runtime.stopping.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    db.update_remote(&cached, &devices, now())?;
    Ok(())
}
fn sync_allowed(runtime: &Runtime) -> Result<bool, String> {
    Ok(!runtime.stopping.load(std::sync::atomic::Ordering::Relaxed)
        && runtime
            .db
            .lock()
            .map_err(|_| "DB lock")?
            .settings()
            .sync_enabled)
}
#[cfg(all(test, windows))]
mod tests;
