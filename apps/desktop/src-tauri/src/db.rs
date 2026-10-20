use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Db {
    pub conn: Connection,
}
impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 2 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS recordings(id TEXT PRIMARY KEY, device_id TEXT NOT NULL, started_at INTEGER NOT NULL, ended_at INTEGER, key_schema_version INTEGER NOT NULL DEFAULT 1, data_generation INTEGER NOT NULL, eligible INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS hourly(recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE, hour_start INTEGER NOT NULL CHECK(hour_start%3600=0), key_code INTEGER NOT NULL, value INTEGER NOT NULL CHECK(value>=0), local_revision INTEGER NOT NULL DEFAULT 1, acked_revision INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(recording_id,hour_start,key_code));
          CREATE TABLE IF NOT EXISTS remote_cache(device_id TEXT NOT NULL, recording_id TEXT NOT NULL, hour_start INTEGER NOT NULL, key_code INTEGER NOT NULL, value INTEGER NOT NULL, PRIMARY KEY(recording_id,hour_start,key_code));
          CREATE INDEX IF NOT EXISTS hourly_hour ON hourly(hour_start);
          ")?;
        if version < 2 {
            conn.execute_batch("BEGIN IMMEDIATE;
          CREATE TABLE hourly_v2(recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,hour_start INTEGER NOT NULL CHECK(hour_start>=0 AND hour_start%3600=0),key_code INTEGER NOT NULL CHECK(key_code BETWEEN -1 AND 127 OR key_code BETWEEN 256 AND 383 OR key_code BETWEEN 512 AND 639),value INTEGER NOT NULL CHECK(value>=0 AND value<=9007199254740991),local_revision INTEGER NOT NULL DEFAULT 1,acked_revision INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(recording_id,hour_start,key_code));
          INSERT INTO hourly_v2 SELECT * FROM hourly;
          DROP TABLE hourly;ALTER TABLE hourly_v2 RENAME TO hourly;CREATE INDEX hourly_hour ON hourly(hour_start);
          PRAGMA user_version=2;COMMIT;")?;
        }
        Ok(Self { conn })
    }
    pub fn get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key=?", [key], |r| {
                r.get(0)
            })
            .ok()
    }
    pub fn cleanup_mode(&self) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key='session_cleanup'",
                [],
                |r| r.get(0),
            )
            .optional()
    }
    pub fn set(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
    pub fn device_id(&self) -> rusqlite::Result<String> {
        if let Some(id) = self.get("device_id") {
            return Ok(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.set("device_id", &id)?;
        Ok(id)
    }
    pub fn settings(&self) -> Settings {
        self.get("preferences")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save_settings(&self, s: &Settings) -> Result<(), String> {
        self.set(
            "preferences",
            &serde_json::to_string(s).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
    pub fn generation(&self) -> i64 {
        self.get("generation")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
    pub fn start(&self, device: &str, eligible: bool) -> rusqlite::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO recordings(id,device_id,started_at,data_generation,eligible) VALUES(?,?,?,?,?)",params![id,device,now(),self.generation(),eligible])?;
        Ok(id)
    }
    pub fn reset_recording(
        &mut self,
        device: &str,
        generation: Option<i64>,
        keep_local: bool,
        disable_sync: bool,
    ) -> Result<String, String> {
        let next_generation = generation.unwrap_or_else(|| self.generation());
        let mut settings = self.settings();
        if disable_sync {
            settings.sync_enabled = false;
        }
        let preferences = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        (||->rusqlite::Result<()>{
            if keep_local{tx.execute("UPDATE recordings SET eligible=0",[])?;}else{tx.execute("DELETE FROM recordings",[])?;}
            tx.execute("DELETE FROM remote_cache",[])?;
            tx.execute("INSERT INTO settings VALUES('generation',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[next_generation.to_string()])?;
            tx.execute("INSERT INTO settings VALUES('preferences',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[preferences])?;
            tx.execute("INSERT INTO recordings(id,device_id,started_at,data_generation,eligible) VALUES(?,?,?,?,?)",params![id,device,now(),next_generation,!disable_sync])?;
            Ok(())
        })().map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(id)
    }
    pub fn disconnect(&mut self, new_device: Option<&str>) -> Result<(), String> {
        let mut settings = self.settings();
        settings.sync_enabled = false;
        settings.upload_history = false;
        let preferences = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        (|| -> rusqlite::Result<()> {
            tx.execute("INSERT INTO settings VALUES('preferences',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [preferences])?;
            tx.execute("UPDATE recordings SET eligible=0", [])?;
            tx.execute("DELETE FROM remote_cache", [])?;
            tx.execute("DELETE FROM settings WHERE key IN('account_id','devices','last_sync')", [])?;
            if let Some(device) = new_device {
                tx.execute("INSERT INTO settings VALUES('device_id',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [device])?;
            }
            // Commit the intent before touching DPAPI files. Startup must ignore the old session.
            tx.execute("INSERT INTO settings VALUES('session_cleanup',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if new_device.is_some() { "replace_device" } else { "logout" }])?;
            Ok(())
        })().map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn flush(&mut self, rows: &[Row]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for r in rows {
            tx.execute("INSERT INTO hourly(recording_id,hour_start,key_code,value) VALUES(?,?,?,?) ON CONFLICT(recording_id,hour_start,key_code) DO UPDATE SET value=MIN(9007199254740991,hourly.value+excluded.value),local_revision=local_revision+1",params![r.recording_id,r.hour_start,r.key_code.map(i64::from).unwrap_or(-1),r.value])?;
        }
        tx.execute("INSERT INTO settings VALUES('last_saved',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[now().to_string()])?;
        tx.commit()
    }
    pub fn rows(&self) -> rusqlite::Result<Vec<StatsRow>> {
        let mut st=self.conn.prepare("SELECT r.device_id,h.recording_id,h.hour_start,h.key_code,h.value FROM hourly h JOIN recordings r ON r.id=h.recording_id UNION ALL SELECT device_id,recording_id,hour_start,key_code,value FROM remote_cache")?;
        let rows = st
            .query_map([], |r| {
                let key: i64 = r.get(3)?;
                Ok(StatsRow {
                    device_id: r.get(0)?,
                    recording_id: r.get(1)?,
                    hour_start: r.get(2)?,
                    key_code: if key < 0 { None } else { Some(key as u16) },
                    value: r.get(4)?,
                })
            })?
            .collect();
        rows
    }
    pub fn pending(&self) -> rusqlite::Result<(Vec<Recording>, Vec<Row>)> {
        let mut st=self.conn.prepare("SELECT h.recording_id,h.hour_start,h.key_code,h.value,h.local_revision FROM hourly h JOIN recordings r ON r.id=h.recording_id WHERE r.eligible=1 AND r.data_generation=? AND h.local_revision>h.acked_revision ORDER BY h.hour_start LIMIT 20")?;
        let rows: Vec<Row> = st
            .query_map([self.generation()], |r| {
                let k: i64 = r.get(2)?;
                Ok(Row {
                    recording_id: r.get(0)?,
                    hour_start: r.get(1)?,
                    key_code: if k < 0 { None } else { Some(k as u16) },
                    value: r.get(3)?,
                    revision: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut recordings = Vec::new();
        for row in &rows {
            if recordings
                .iter()
                .any(|r: &Recording| r.id == row.recording_id)
            {
                continue;
            }
            recordings.push(self.conn.query_row("SELECT id,started_at,ended_at,key_schema_version,data_generation FROM recordings WHERE id=?",[&row.recording_id],|r|Ok(Recording{id:r.get(0)?,started_at:r.get(1)?,ended_at:r.get(2)?,key_schema_version:r.get(3)?,data_generation:r.get(4)?}))?);
        }
        Ok((recordings, rows))
    }
    pub fn ack(&mut self, rows: &[Row]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for r in rows {
            tx.execute("UPDATE hourly SET acked_revision=MAX(acked_revision,?) WHERE recording_id=? AND hour_start=? AND key_code=?",params![r.revision,r.recording_id,r.hour_start,r.key_code.map(i64::from).unwrap_or(-1)])?;
        }
        tx.commit()
    }
    pub fn restore(
        &mut self,
        records: &[Recording],
        rows: &[StatsRow],
        device: &str,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for r in records {
            tx.execute("INSERT INTO recordings(id,device_id,started_at,ended_at,key_schema_version,data_generation,eligible) VALUES(?,?,?,?,?,?,1) ON CONFLICT(id) DO NOTHING",params![r.id,device,r.started_at,r.ended_at,r.key_schema_version,r.data_generation])?;
        }
        for r in rows {
            tx.execute("INSERT INTO hourly(recording_id,hour_start,key_code,value,local_revision,acked_revision) VALUES(?,?,?,?,0,0) ON CONFLICT(recording_id,hour_start,key_code) DO UPDATE SET value=MAX(hourly.value,excluded.value)",params![r.recording_id,r.hour_start,r.key_code.map(i64::from).unwrap_or(-1),r.value])?;
        }
        tx.commit()
    }
    #[cfg(test)]
    pub fn cache(&mut self, rows: &[StatsRow]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        Self::write_cache(&tx, rows)?;
        tx.commit()
    }
    fn write_cache(tx: &rusqlite::Transaction<'_>, rows: &[StatsRow]) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM remote_cache", [])?;
        for r in rows {
            tx.execute(
                "INSERT INTO remote_cache VALUES(?,?,?,?,?)",
                params![
                    r.device_id,
                    r.recording_id,
                    r.hour_start,
                    r.key_code.map(i64::from).unwrap_or(-1),
                    r.value
                ],
            )?;
        }
        Ok(())
    }
    pub fn update_remote(
        &mut self,
        rows: &[StatsRow],
        devices: &[Device],
        synchronized_at: i64,
    ) -> Result<(), String> {
        let devices = serde_json::to_string(devices).map_err(|e| e.to_string())?;
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        Self::write_cache(&tx, rows).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO settings VALUES('devices',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[devices]).map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO settings VALUES('last_sync',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[synchronized_at.to_string()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "launched by the parent crash recovery test"]
    fn crash_child() {
        let Some(path) = std::env::var_os("MOUSE_STATS_CRASH_TEST_PATH") else {
            return;
        };
        let mut db = Db::open(Path::new(&path)).unwrap();
        let device = db.device_id().unwrap();
        let recording = db.start(&device, true).unwrap();
        db.flush(&[Row {
            recording_id: recording,
            hour_start: 0,
            key_code: Some(30),
            value: 4,
            revision: 0,
        }])
        .unwrap();
        db.conn
            .execute_batch("BEGIN IMMEDIATE; UPDATE hourly SET value=99;")
            .unwrap();
        // Skip Connection::drop, rollback and checkpoint, as an unexpected process exit would.
        std::process::exit(17);
    }
    #[test]
    fn wal_recovers_committed_totals_after_process_exit_without_cleanup() {
        let directory =
            std::env::temp_dir().join(format!("mouse-stats-crash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("statistics.db");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "db::tests::crash_child", "--ignored"])
            .env("MOUSE_STATS_CRASH_TEST_PATH", &path)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(17));
        {
            let mut db = Db::open(&path).unwrap();
            assert_eq!(db.rows().unwrap()[0].value, 4);
            let device = db.device_id().unwrap();
            let recording = db.start(&device, true).unwrap();
            db.flush(&[Row {
                recording_id: recording,
                hour_start: 0,
                key_code: Some(30),
                value: 3,
                revision: 0,
            }])
            .unwrap();
            assert_eq!(db.rows().unwrap().iter().map(|r| r.value).sum::<i64>(), 7);
        }
        for file in ["statistics.db", "statistics.db-wal", "statistics.db-shm"] {
            let file = directory.join(file);
            if file.exists() {
                std::fs::remove_file(file).unwrap();
            }
        }
        std::fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn failed_generation_reset_preserves_statistics_and_settings() {
        let mut db = Db::open(Path::new(":memory:")).unwrap();
        db.save_settings(&Settings {
            sync_enabled: true,
            ..Default::default()
        })
        .unwrap();
        let d = db.device_id().unwrap();
        let id = db.start(&d, true).unwrap();
        db.flush(&[Row {
            recording_id: id,
            hour_start: 0,
            key_code: Some(30),
            value: 4,
            revision: 0,
        }])
        .unwrap();
        db.conn.execute_batch("CREATE TRIGGER reject_recording BEFORE INSERT ON recordings BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        assert!(db.reset_recording(&d, Some(1), false, true).is_err());
        assert_eq!(db.generation(), 0);
        assert!(db.settings().sync_enabled);
        assert_eq!(db.rows().unwrap()[0].value, 4);
        db.conn
            .execute_batch("DROP TRIGGER reject_recording")
            .unwrap();
        let new = db.reset_recording(&d, Some(1), false, false).unwrap();
        assert_eq!(db.generation(), 1);
        assert!(db.rows().unwrap().is_empty());
        assert!(db
            .conn
            .query_row("SELECT eligible FROM recordings WHERE id=?", [new], |r| {
                r.get::<_, bool>(0)
            })
            .unwrap());
    }
    #[test]
    fn migration_preserves_data_and_future_schema_is_rejected() {
        let path =
            std::env::temp_dir().join(format!("mouse-stats-migration-{}.db", uuid::Uuid::new_v4()));
        {
            let mut db = Db::open(&path).unwrap();
            let d = db.device_id().unwrap();
            let id = db.start(&d, true).unwrap();
            db.flush(&[Row {
                recording_id: id,
                hour_start: 0,
                key_code: Some(30),
                value: 3,
                revision: 0,
            }])
            .unwrap();
            db.conn.pragma_update(None, "user_version", 1).unwrap();
        }
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.rows().unwrap()[0].value, 3);
            assert_eq!(
                db.conn
                    .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                2
            );
            db.conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(Db::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn failed_batch_is_atomic_including_save_timestamp() {
        let mut db = Db::open(Path::new(":memory:")).unwrap();
        let d = db.device_id().unwrap();
        let id = db.start(&d, true).unwrap();
        let valid = Row {
            recording_id: id.clone(),
            hour_start: 0,
            key_code: Some(30),
            value: 1,
            revision: 0,
        };
        let invalid = Row {
            recording_id: id,
            hour_start: 0,
            key_code: Some(31),
            value: -1,
            revision: 0,
        };
        assert!(db.flush(&[valid, invalid]).is_err());
        assert!(db.rows().unwrap().is_empty());
        assert!(db.get("last_saved").is_none());
    }
    #[test]
    fn restart_and_ack_race() {
        let mut db = Db::open(Path::new(":memory:")).unwrap();
        let d = db.device_id().unwrap();
        let a = db.start(&d, true).unwrap();
        let row = Row {
            recording_id: a,
            hour_start: 0,
            key_code: Some(30),
            value: 4,
            revision: 0,
        };
        db.flush(std::slice::from_ref(&row)).unwrap();
        let (_, sent) = db.pending().unwrap();
        db.flush(&[row]).unwrap();
        db.ack(&sent).unwrap();
        assert_eq!(db.pending().unwrap().1[0].value, 8);
        let b = db.start(&d, true).unwrap();
        db.flush(&[Row {
            recording_id: b,
            hour_start: 0,
            key_code: Some(30),
            value: 3,
            revision: 0,
        }])
        .unwrap();
        assert_eq!(db.rows().unwrap().iter().map(|r| r.value).sum::<i64>(), 11);
    }
    #[test]
    fn cloud_restore_is_max_and_current_device_not_cached() {
        let mut db = Db::open(Path::new(":memory:")).unwrap();
        let d = db.device_id().unwrap();
        let id = db.start(&d, true).unwrap();
        let record = Recording {
            id: id.clone(),
            started_at: 0,
            ended_at: None,
            key_schema_version: 1,
            data_generation: 0,
        };
        let row = StatsRow {
            device_id: d.clone(),
            recording_id: id,
            hour_start: 0,
            key_code: None,
            value: 5000,
        };
        db.restore(
            std::slice::from_ref(&record),
            std::slice::from_ref(&row),
            &d,
        )
        .unwrap();
        db.restore(&[record], &[row], &d).unwrap();
        assert_eq!(db.rows().unwrap()[0].value, 5000);
    }
}
