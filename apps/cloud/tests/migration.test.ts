import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, it, expect } from "vite-plus/test";

describe("hourly validation migration", () => {
  it("preserves existing aggregate rows while rebuilding the table", () => {
    const dir = fileURLToPath(new URL("../db/migrations/", import.meta.url));
    const db = new DatabaseSync(":memory:");
    try {
      db.exec(readFileSync(`${dir}20261019094637_gifted_thanos.sql`, "utf8"));
      db.exec(`
        INSERT INTO users(id, username, data_generation, created_at) VALUES ('u', 'alice', 0, 0);
        INSERT INTO devices(id, user_id, name, created_at, last_seen, revoked_at)
          VALUES ('d', 'u', 'PC A', 0, 0, NULL);
        INSERT INTO recordings(id, device_id, data_generation, started_at, ended_at, key_schema_version)
          VALUES ('r', 'd', 0, 0, NULL, 1);
        INSERT INTO hourly(recording_id, hour_start, key_code, value) VALUES ('r', 3600, 4, 12);
      `);

      db.exec(readFileSync(`${dir}20261019103925_lethal_sentry.sql`, "utf8"));

      expect(
        db.prepare("SELECT recording_id, hour_start, key_code, value FROM hourly").all(),
      ).toEqual([{ recording_id: "r", hour_start: 3600, key_code: 4, value: 12 }]);
      expect(() =>
        db
          .prepare(
            "INSERT INTO hourly(recording_id, hour_start, key_code, value) VALUES ('r', 3601, 4, 1)",
          )
          .run(),
      ).toThrow();
    } finally {
      db.close();
    }
  });
});
