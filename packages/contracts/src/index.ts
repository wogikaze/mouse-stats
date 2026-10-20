import { z } from "zod";

export const KEY_SCHEMA_VERSION = 1;
export const MAX_BATCH = 20;
export const MAX_VALUE = Number.MAX_SAFE_INTEGER;
export const keyCode = z
  .number()
  .int()
  .min(0)
  .max(0x27f)
  .refine((v) => (v & 0xff) <= 0x7f && (v < 0x80 || (v >= 0x100 && v <= 0x17f) || v >= 0x200));
const integer = z.number().int().nonnegative().max(MAX_VALUE);
export const hourStart = integer.refine((v) => v % 3600 === 0);
export const rowSchema = z
  .object({
    recording_id: z.uuid(),
    hour_start: hourStart,
    key_code: keyCode.nullable(),
    value: integer,
    revision: integer,
  })
  .strict();
export const recordingSchema = z
  .object({
    id: z.uuid(),
    started_at: integer,
    ended_at: integer.nullable(),
    key_schema_version: z.literal(1),
    data_generation: integer,
  })
  .strict();
export const pushSchema = z
  .object({
    generation: integer,
    recordings: z.array(recordingSchema).max(MAX_BATCH),
    rows: z.array(rowSchema).max(MAX_BATCH),
  })
  .strict();
export type HourlyRow = z.infer<typeof rowSchema>;
export type Recording = z.infer<typeof recordingSchema>;
export type Push = z.infer<typeof pushSchema>;
export interface StatsRow {
  device_id: string;
  recording_id: string;
  hour_start: number;
  key_code: number | null;
  value: number;
}
export interface Device {
  id: string;
  name: string;
  last_seen: number;
  revoked_at: number | null;
}
export interface Settings {
  consent: boolean;
  paused: boolean;
  autostart: boolean;
  sync_enabled: boolean;
  upload_history: boolean;
  timezone: string;
  layout: "US" | "JIS";
  cloud_url: string;
}
export interface Snapshot {
  device_id: string;
  recording_id: string;
  settings: Settings;
  rows: StatsRow[];
  devices: Device[];
  last_saved: number | null;
  last_sync: number | null;
  sync_status: string;
  error: string | null;
  generation_conflict: boolean;
  signed_in: boolean;
}

// Set 1 scan code + E0=0x100 / E1=0x200. Code 0 represents Unknown.
export interface PhysicalKey {
  code: number;
  label: string;
  width?: number;
}
const row = (codes: number[], labels: string[]): PhysicalKey[] =>
  codes.map((code, i) => ({ code, label: labels[i]! }));
export function keyboard(layout: "US" | "JIS"): PhysicalKey[][] {
  const rows = [
    row(
      [1, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 0x137, 70, 0x245],
      [
        "Esc",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
        "PrtSc",
        "ScrLk",
        "Pause",
      ],
    ),
    row(
      [41, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 0x152, 0x147, 0x149],
      [
        "`",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "0",
        "-",
        "=",
        "Back",
        "Ins",
        "Home",
        "PgUp",
      ],
    ),
    row(
      [15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 43, 0x153, 0x14f, 0x151],
      [
        "Tab",
        "Q",
        "W",
        "E",
        "R",
        "T",
        "Y",
        "U",
        "I",
        "O",
        "P",
        "[",
        "]",
        "\\",
        "Del",
        "End",
        "PgDn",
      ],
    ),
    row(
      [58, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 28],
      ["Caps", "A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'", "Enter"],
    ),
    row(
      [42, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 0x148],
      ["Shift L", "Z", "X", "C", "V", "B", "N", "M", ",", ".", "/", "Shift R", "↑"],
    ),
    row(
      [29, 0x15b, 56, 57, 0x138, 0x15c, 0x15d, 0x11d, 0x14b, 0x150, 0x14d],
      ["Ctrl L", "Win L", "Alt L", "Space", "Alt R", "Win R", "Menu", "Ctrl R", "←", "↓", "→"],
    ),
    row(
      [69, 0x135, 55, 74, 71, 72, 73, 78, 75, 76, 77, 79, 80, 81, 82, 83, 0x11c],
      [
        "Num",
        "N /",
        "N *",
        "N -",
        "N 7",
        "N 8",
        "N 9",
        "N +",
        "N 4",
        "N 5",
        "N 6",
        "N 1",
        "N 2",
        "N 3",
        "N 0",
        "N .",
        "N Enter",
      ],
    ),
  ];
  if (layout === "JIS") {
    rows[1]![0]!.label = "半角";
    rows[1]!.splice(13, 0, { code: 125, label: "¥" });
    rows[1]!.find((k) => k.code === 13)!.label = "^";
    rows[2]!.find((k) => k.code === 26)!.label = "@";
    rows[2]!.find((k) => k.code === 27)!.label = "[";
    rows[2] = rows[2]!.filter((k) => k.code !== 43);
    rows[3]!.splice(rows[3]!.length - 1, 0, { code: 43, label: "]" });
    rows[3]!.find((k) => k.code === 40)!.label = ":";
    rows[4]!.splice(11, 0, { code: 115, label: "ろ" });
    rows[5]!.splice(3, 0, { code: 123, label: "無変換" });
    rows[5]!.splice(5, 0, { code: 121, label: "変換" }, { code: 112, label: "かな" });
  }
  const widths: Record<number, number> = {
    14: layout === "JIS" ? 1 : 2,
    15: 1.5,
    43: layout === "JIS" ? 1 : 1.5,
    58: 1.75,
    28: layout === "JIS" ? 1.25 : 2.25,
    42: 2.25,
    54: layout === "JIS" ? 1.75 : 2.75,
    57: layout === "JIS" ? 2.5 : 6.25,
    29: 1.25,
    0x15b: 1.25,
    56: 1.25,
    0x138: 1.25,
    0x15c: 1.25,
    0x15d: 1.25,
    0x11d: 1.25,
    123: 1.25,
    121: 1.25,
    112: 1.25,
  };
  return rows.map((r) => r.map((k) => ({ ...k, width: widths[k.code] ?? 1 })));
}
export function localDay(epoch: number, timezone: string): string {
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone: timezone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(new Date(epoch * 1000));
  const get = (type: string) => parts.find((p) => p.type === type)!.value;
  return `${get("year")}-${get("month")}-${get("day")}`;
}
export function summarize(rows: StatsRow[], timezone: string, from = "", to = "9999", device = "") {
  const days = new Map<string, { keys: number; mouse: number }>();
  const keys = new Map<number, number>();
  const hours = new Map<number, { keys: number; mouse: number; label: string }>();
  let presses = 0,
    mouse = 0;
  for (const r of rows) {
    if (device && r.device_id !== device) continue;
    const day = localDay(r.hour_start, timezone);
    if (day < from || day > to) continue;
    const d = days.get(day) ?? { keys: 0, mouse: 0 };
    const h = hours.get(r.hour_start) ?? {
      keys: 0,
      mouse: 0,
      label: new Intl.DateTimeFormat("ja-JP", {
        timeZone: timezone,
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        timeZoneName: "shortOffset",
      }).format(new Date(r.hour_start * 1000)),
    };
    if (r.key_code === null) {
      mouse += r.value;
      d.mouse += r.value;
      h.mouse += r.value;
    } else {
      presses += r.value;
      d.keys += r.value;
      h.keys += r.value;
      keys.set(r.key_code, (keys.get(r.key_code) ?? 0) + r.value);
    }
    days.set(day, d);
    hours.set(r.hour_start, h);
  }
  return { presses, mouse, days, keys, hours };
}
