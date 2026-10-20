import { describe, it, expect } from "vite-plus/test";
import { cleanupExpired } from "../src/api";
import { testDatabase } from "./d1";

describe("scheduled expiry cleanup", () => {
  it("removes expired challenges and rate-limit buckets while retaining live rows", async () => {
    const db = testDatabase();
    try {
      await db
        .prepare("INSERT INTO challenges VALUES(?,?,?,?,?)")
        .bind("old-challenge", "challenge", "login", "{}", 99)
        .run();
      await db
        .prepare("INSERT INTO challenges VALUES(?,?,?,?,?)")
        .bind("boundary-challenge", "challenge", "login", "{}", 100)
        .run();
      await db
        .prepare("INSERT INTO challenges VALUES(?,?,?,?,?)")
        .bind("live-challenge", "challenge", "login", "{}", 101)
        .run();
      await db.prepare("INSERT INTO rate_limits VALUES(?,?,?)").bind("old", 1, 99).run();
      await db.prepare("INSERT INTO rate_limits VALUES(?,?,?)").bind("boundary", 1, 100).run();
      await db.prepare("INSERT INTO rate_limits VALUES(?,?,?)").bind("live", 1, 101).run();

      await cleanupExpired(db, 100);

      expect(await db.prepare("SELECT id FROM challenges").all()).toMatchObject({
        results: [{ id: "live-challenge" }],
      });
      expect(await db.prepare("SELECT key FROM rate_limits").all()).toMatchObject({
        results: [{ key: "live" }],
      });
    } finally {
      db.close();
    }
  });
});
