use crate::model::{hour, Row, MAX_VALUE};
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
pub struct Aggregator {
    held: HashSet<(usize, u16)>,
    pending: BTreeMap<(i64, Option<u16>), i64>,
    pub paused: bool,
    pub clock_warning: bool,
    last_time: Option<(i64, std::time::Instant)>,
}
pub fn normalize(scan: u16, e0: bool, e1: bool, vkey: u16) -> Option<u16> {
    // PrintScreen fake shift packets and keyboard overrun do not represent keys.
    if scan == 0xff || vkey == 0xff || (e0 && matches!(scan, 0x2a | 0x36)) {
        return None;
    }
    if vkey == 0x2c && matches!(scan, 0x37 | 0x54) {
        // Alt+PrintScreen emits SysRq (0x54) for the same physical key.
        return Some(0x137);
    }
    if vkey == 0x90 && scan == 0x45 {
        return Some(0x45);
    }
    if vkey == 0x13 || vkey == 0x03 && e0 && scan == 0x46 {
        return Some(0x245);
    } // Pause: Windows often omits key-up.
    if scan == 0 || scan > 0x7f {
        return Some(0);
    }
    Some(
        scan | if e1 {
            0x200
        } else if e0 {
            0x100
        } else {
            0
        },
    )
}
impl Aggregator {
    pub fn new(paused: bool) -> Self {
        Self {
            paused,
            ..Default::default()
        }
    }
    fn add(&mut self, t: i64, key: Option<u16>, value: i64) {
        if self.paused {
            return;
        }
        if let Some((prev, instant)) = self.last_time {
            if (t - prev - instant.elapsed().as_secs() as i64).abs() > 60 {
                self.clock_warning = true;
            }
        }
        self.last_time = Some((t, std::time::Instant::now()));
        let count = self.pending.entry((hour(t), key)).or_default();
        *count = count.saturating_add(value).min(MAX_VALUE);
    }
    pub fn key(&mut self, device: usize, code: u16, down: bool, t: i64) {
        if down {
            if self.held.insert((device, code)) {
                self.add(t, Some(code), 1);
            }
            if code == 0x245 {
                self.held.remove(&(device, code));
            }
        } else {
            self.held.remove(&(device, code));
        }
    }
    pub fn mouse(&mut self, dx: i32, dy: i32, absolute: bool, t: i64) {
        if !absolute {
            self.add(
                t,
                None,
                ((dx as f64).hypot(dy as f64) * 1000.).round() as i64,
            );
        }
    }
    pub fn reset_held(&mut self) {
        self.held.clear();
    }
    pub fn release_device(&mut self, device: usize) {
        self.held.retain(|(d, _)| *d != device);
    }
    pub fn release_code(&mut self, code: u16) {
        self.held.retain(|(_, c)| *c != code);
    }
    pub fn drain(&mut self, recording: &str) -> Vec<Row> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|((hour_start, key_code), value)| Row {
                recording_id: recording.into(),
                hour_start,
                key_code,
                value,
                revision: 0,
            })
            .collect()
    }
    pub fn restore(&mut self, rows: Vec<Row>) {
        for r in rows {
            let v = self.pending.entry((r.hour_start, r.key_code)).or_default();
            *v = v.saturating_add(r.value).min(MAX_VALUE);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_resume_does_not_turn_a_held_key_into_a_new_press() {
        let mut a = Aggregator::default();
        a.key(1, 30, true, 1);
        a.paused = true;
        a.key(1, 30, true, 2);
        a.paused = false;
        a.key(1, 30, true, 3);
        a.key(1, 30, false, 4);
        a.key(1, 30, true, 5);
        assert_eq!(a.drain("r")[0].value, 2);
    }
    #[test]
    fn high_rate_input_is_aggregated_without_event_retention() {
        let mut a = Aggregator::default();
        for _ in 0..100_000 {
            a.mouse(3, 4, false, 1234);
        }
        let rows = a.drain("r");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, 500_000_000);
    }
    #[test]
    fn repeat_and_multiple_keyboards() {
        let mut a = Aggregator::default();
        a.key(1, 30, true, 3599);
        a.key(1, 30, true, 3599);
        a.key(2, 30, true, 3599);
        a.key(1, 30, false, 3599);
        a.key(1, 30, true, 3600);
        let r = a.drain("r");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].value, 2);
        assert_eq!(r[1].value, 1);
    }
    #[test]
    fn mouse_units_absolute_pause() {
        let mut a = Aggregator::default();
        a.mouse(3, 4, false, 1);
        a.mouse(300, 400, true, 1);
        a.paused = true;
        a.mouse(3, 4, false, 1);
        assert_eq!(a.drain("r")[0].value, 5000);
    }
    #[test]
    fn physical_keys() {
        assert_eq!(normalize(29, true, false, 0x11), Some(0x11d));
        assert_eq!(normalize(125, false, false, 0xdc), Some(125));
        assert_eq!(normalize(42, true, false, 0xff), None);
        assert_eq!(normalize(69, false, true, 0x13), Some(0x245));
        assert_eq!(normalize(0x54, false, false, 0x2c), Some(0x137));
        assert_eq!(normalize(0x37, true, false, 0x2c), Some(0x137));
        assert_eq!(normalize(0x45, true, false, 0x90), Some(0x45));
        assert_eq!(normalize(0x46, true, false, 0x03), Some(0x245));
    }
    #[test]
    fn lost_sysrq_printscreen_key_up_is_released_by_the_normalized_scan_code() {
        let mut a = Aggregator::default();
        let physical = crate::input::timer_release_code(0x54, false, 0x2c).unwrap();
        a.key(7, physical, true, 1);
        // The timer path and WM_INPUT share the same normalization rule.
        a.release_code(physical);
        a.key(7, physical, true, 2);
        assert_eq!(a.drain("r")[0].value, 2);
    }
    #[test]
    fn failed_flush_restores() {
        let mut a = Aggregator::default();
        a.mouse(3, 4, false, 1);
        let r = a.drain("r");
        a.mouse(0, 1, false, 1);
        a.restore(r);
        assert_eq!(a.drain("r")[0].value, 6000);
    }
}
