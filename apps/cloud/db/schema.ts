import { sqliteTable, text, integer, primaryKey, index, check } from "drizzle-orm/sqlite-core";
import { sql } from "drizzle-orm";
export const users = sqliteTable("users", {
  id: text().primaryKey(),
  username: text().notNull().unique(),
  data_generation: integer().notNull().default(0),
  created_at: integer().notNull(),
});
export const passkeys = sqliteTable("passkeys", {
  credential_id: text().primaryKey(),
  user_id: text()
    .notNull()
    .references(() => users.id, { onDelete: "cascade" }),
  public_key: text().notNull(),
  counter: integer().notNull(),
  transports: text().notNull(),
  name: text().notNull(),
  created_at: integer().notNull(),
  last_used: integer(),
  revoked_at: integer(),
});
export const devices = sqliteTable("devices", {
  id: text().primaryKey(),
  user_id: text()
    .notNull()
    .references(() => users.id, { onDelete: "cascade" }),
  name: text().notNull(),
  created_at: integer().notNull(),
  last_seen: integer().notNull(),
  revoked_at: integer(),
  recovery_key: text().notNull().default(""),
});
export const sessions = sqliteTable("device_sessions", {
  id: text().primaryKey(),
  user_id: text()
    .notNull()
    .references(() => users.id, { onDelete: "cascade" }),
  device_id: text().references(() => devices.id, { onDelete: "cascade" }),
  refresh_hash: text().unique(),
  access_hash: text().notNull().unique(),
  access_expires: integer().notNull(),
  expires_at: integer().notNull(),
  revoked_at: integer(),
  stepup_until: integer().notNull().default(0),
  previous_refresh_hash: text(),
  rotation_nonce_hash: text(),
});
export const recovery = sqliteTable("recovery_codes", {
  id: text().primaryKey(),
  user_id: text()
    .notNull()
    .references(() => users.id, { onDelete: "cascade" }),
  code_hash: text().notNull().unique(),
  created_at: integer().notNull(),
  used_at: integer(),
});
export const recordings = sqliteTable(
  "recordings",
  {
    id: text().primaryKey(),
    device_id: text()
      .notNull()
      .references(() => devices.id, { onDelete: "cascade" }),
    data_generation: integer().notNull(),
    started_at: integer().notNull(),
    ended_at: integer(),
    key_schema_version: integer().notNull(),
  },
  (t) => [index("recordings_device").on(t.device_id)],
);
// key_code=-1 represents the mouse bucket; no raw events or event ordering exist.
export const hourly = sqliteTable(
  "hourly",
  {
    recording_id: text()
      .notNull()
      .references(() => recordings.id, { onDelete: "cascade" }),
    hour_start: integer().notNull(),
    key_code: integer().notNull(),
    value: integer().notNull(),
  },
  (t) => [
    primaryKey({ columns: [t.recording_id, t.hour_start, t.key_code] }),
    index("hourly_hour").on(t.hour_start),
    check("hourly_value_range", sql`${t.value}>=0 AND ${t.value}<=9007199254740991`),
    check("hourly_utc", sql`${t.hour_start}>=0 AND ${t.hour_start}%3600=0`),
    check(
      "hourly_physical_key",
      sql`${t.key_code} BETWEEN -1 AND 127 OR ${t.key_code} BETWEEN 256 AND 383 OR ${t.key_code} BETWEEN 512 AND 639`,
    ),
  ],
);
export const challenges = sqliteTable("challenges", {
  id: text().primaryKey(),
  challenge: text().notNull(),
  purpose: text().notNull(),
  context: text().notNull(),
  expires_at: integer().notNull(),
});
export const codes = sqliteTable("authorization_codes", {
  code_hash: text().primaryKey(),
  user_id: text()
    .notNull()
    .references(() => users.id, { onDelete: "cascade" }),
  pkce: text().notNull(),
  state: text().notNull(),
  expires_at: integer().notNull(),
});
export const limits = sqliteTable("rate_limits", {
  key: text().primaryKey(),
  count: integer().notNull(),
  expires_at: integer().notNull(),
});
