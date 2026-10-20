use serde::{Deserialize, Serialize};

pub const MAX_VALUE: i64 = 9_007_199_254_740_991;
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn hour(t: i64) -> i64 {
    t.div_euclid(3600) * 3600
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Row {
    pub recording_id: String,
    pub hour_start: i64,
    pub key_code: Option<u16>,
    pub value: i64,
    pub revision: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub key_schema_version: u8,
    pub data_generation: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatsRow {
    pub device_id: String,
    pub recording_id: String,
    pub hour_start: i64,
    pub key_code: Option<u16>,
    pub value: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub last_seen: i64,
    pub revoked_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub consent: bool,
    pub paused: bool,
    pub autostart: bool,
    pub sync_enabled: bool,
    pub upload_history: bool,
    pub timezone: String,
    pub layout: String,
    pub cloud_url: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            consent: false,
            paused: false,
            autostart: false,
            sync_enabled: false,
            upload_history: false,
            timezone: String::new(),
            layout: "US".into(),
            cloud_url: String::new(),
        }
    }
}
#[derive(Serialize)]
pub struct Snapshot {
    pub device_id: String,
    pub recording_id: String,
    pub settings: Settings,
    pub rows: Vec<StatsRow>,
    pub devices: Vec<Device>,
    pub last_saved: Option<i64>,
    pub last_sync: Option<i64>,
    pub sync_status: String,
    pub error: Option<String>,
    pub generation_conflict: bool,
    pub signed_in: bool,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_contract_matches_typescript() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../packages/contracts/fixtures/wire.json"
        ))
        .unwrap();
        let rows: Vec<Row> = serde_json::from_value(value["rows"].clone()).unwrap();
        let recordings: Vec<Recording> =
            serde_json::from_value(value["recordings"].clone()).unwrap();
        assert_eq!(serde_json::to_value(rows).unwrap(), value["rows"]);
        assert_eq!(
            serde_json::to_value(recordings).unwrap(),
            value["recordings"]
        );
    }
}
