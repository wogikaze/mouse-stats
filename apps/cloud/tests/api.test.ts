import { beforeEach, afterEach, describe, it, expect } from "vite-plus/test";
import { createApi, hash, type Env } from "../src/api";
import { testDatabase } from "./d1";

describe("production sync and authentication SQL", () => {
  let db: ReturnType<typeof testDatabase>;
  let env: Env;
  let identity: CryptoKeyPair;
  let recoveryKey: string;
  const app = createApi();
  const user = crypto.randomUUID(),
    device = crypto.randomUUID(),
    recording = crypto.randomUUID();
  beforeEach(async () => {
    db = testDatabase();
    env = { DB: db, ORIGIN: "http://localhost", RP_ID: "localhost" };
    identity = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
      "sign",
      "verify",
    ]);
    recoveryKey = Buffer.from(await crypto.subtle.exportKey("raw", identity.publicKey)).toString(
      "base64url",
    );
    await db.prepare("INSERT INTO users VALUES(?,?,0,0)").bind(user, "alice").run();
    await db
      .prepare(
        "INSERT INTO devices(id,user_id,name,created_at,last_seen,revoked_at) VALUES(?,?,?,0,0,NULL)",
      )
      .bind(device, user, "PC A")
      .run();
    await db
      .prepare("UPDATE devices SET recovery_key=? WHERE id=?")
      .bind(recoveryKey, device)
      .run();
    await db
      .prepare(
        "INSERT INTO device_sessions(id,user_id,device_id,refresh_hash,access_hash,access_expires,expires_at,revoked_at,stepup_until) VALUES(?,?,?,?,?,9999999999,9999999999,NULL,9999999999)",
      )
      .bind("session", user, device, await hash("refresh"), await hash("access"))
      .run();
  });
  afterEach(() => db.close());
  const req = (path: string, body?: unknown, method = body ? "POST" : "GET", auth = "access") =>
    app.request(
      `http://localhost/api/v1${path}`,
      {
        method,
        headers: { Authorization: `Bearer ${auth}`, "Content-Type": "application/json" },
        ...(body ? { body: JSON.stringify(body) } : {}),
      },
      env,
    );
  const batch = (value = 10, generation = 0) => ({
    generation,
    recordings: [
      {
        id: recording,
        started_at: 0,
        ended_at: null,
        key_schema_version: 1,
        data_generation: generation,
      },
    ],
    rows: [{ recording_id: recording, hour_start: 0, key_code: 30, value, revision: 1 }],
  });
  it("rate limits normal API and authentication calls per IP and minute window", async () => {
    let last: Response | undefined;
    for (let i = 0; i < 241; i++) last = await req("/health");
    expect(last?.status).toBe(429);
    expect(await last?.json()).toEqual({ error: "RATE_LIMIT" });

    let authLast: Response | undefined;
    for (let i = 0; i < 61; i++)
      authLast = await req("/auth/register/options", {
        username: "alice",
        state: "s".repeat(43),
        pkce: "p".repeat(43),
      });
    expect(authLast?.status).toBe(429);
    expect(await authLast?.json()).toEqual({ error: "RATE_LIMIT" });
  });
  it("MAX upsert ignores repeated, reordered snapshots; new recordings sum", async () => {
    for (const value of [10, 10, 8, 15, 11])
      expect((await req("/sync/push", batch(value))).status).toBe(200);
    const b = batch(4);
    b.recordings[0]!.id = crypto.randomUUID();
    b.rows[0]!.recording_id = b.recordings[0]!.id;
    expect((await req("/sync/push", b)).status).toBe(200);
    const v = await (await req("/sync/pull")).json();
    expect(v.rows.reduce((sum: number, r: { value: number }) => sum + r.value, 0)).toBe(19);
  });
  it("rejects ownership changes and invalid key IDs / values / UTC alignment", async () => {
    await req("/sync/push", batch());
    const other = crypto.randomUUID();
    await db
      .prepare(
        "INSERT INTO devices(id,user_id,name,created_at,last_seen,revoked_at) VALUES(?,?,?,0,0,NULL)",
      )
      .bind(other, user, "PC B")
      .run();
    await db.prepare("UPDATE recordings SET device_id=? WHERE id=?").bind(other, recording).run();
    expect((await req("/sync/push", batch(99))).status).toBe(403);
    for (const change of [
      { key_code: 128 },
      { hour_start: 1 },
      { value: -1 },
      { value: 1.5 },
      { value: Number.MAX_SAFE_INTEGER + 1 },
    ]) {
      const b = batch();
      Object.assign(b.rows[0]!, change);
      expect((await req("/sync/push", b)).status).toBe(400);
    }
  });
  it("combines two PCs and excludes local device from remote cache query", async () => {
    await req("/sync/push", batch(10));
    const b = crypto.randomUUID(),
      r = crypto.randomUUID();
    await db
      .prepare(
        "INSERT INTO devices(id,user_id,name,created_at,last_seen,revoked_at) VALUES(?,?,?,0,0,NULL)",
      )
      .bind(b, user, "PC B")
      .run();
    await db.prepare("INSERT INTO recordings VALUES(?,?,0,0,NULL,1)").bind(r, b).run();
    await db.prepare("INSERT INTO hourly VALUES(?,0,30,7)").bind(r).run();
    const v = await (await req(`/stats?exclude_device=${device}`)).json();
    expect(v.rows).toHaveLength(1);
    expect(v.rows[0].value).toBe(7);
    const own = await (await req("/sync/pull")).json();
    expect(own.rows[0].value).toBe(10);
  });
  it("reset rejects old generation, including an offline device after reconnect", async () => {
    await req("/sync/push", batch());
    expect((await req("/data/reset", {})).status).toBe(200);
    expect((await req("/sync/push", batch())).status).toBe(409);
    expect((await (await req("/stats")).json()).rows).toEqual([]);
    const b = batch(2, 1);
    b.recordings[0]!.id = crypto.randomUUID();
    b.rows[0]!.recording_id = b.recordings[0]!.id;
    expect((await req("/sync/push", b)).status).toBe(200);
  });
  it("revocation immediately blocks session and refresh", async () => {
    expect((await req(`/devices/${device}/revoke`, {})).status).toBe(200);
    expect((await req("/sync/push", batch())).status).toBe(401);
    expect(
      (
        await req("/auth/refresh", {
          refresh_token: "refresh".padEnd(43, "x"),
          nonce: "n".repeat(43),
        })
      ).status,
    ).toBe(401);
  });
  it("refresh rotates atomically; a racing reuse has one winner", async () => {
    const refresh = "R".repeat(43);
    await db
      .prepare("UPDATE device_sessions SET refresh_hash=?")
      .bind(await hash(refresh))
      .run();
    const results = await Promise.all([
      req("/auth/refresh", { refresh_token: refresh, nonce: "n".repeat(43) }),
      req("/auth/refresh", { refresh_token: refresh, nonce: "m".repeat(43) }),
    ]);
    expect(results.map((r) => r.status).sort()).toEqual([200, 401]);
    expect((await req("/stats")).status).toBe(401);
  });
  it("authorization code is PKCE/state bound, short-lived and single use", async () => {
    const code = "c".repeat(43),
      verifier = "v".repeat(43),
      state = "s".repeat(43);
    await db
      .prepare("INSERT INTO authorization_codes VALUES(?,?,?,?,9999999999)")
      .bind(await hash(code), user, await hash(verifier), state)
      .run();
    const b = { code, verifier, state, device_id: device, name: "PC A", recovery_key: recoveryKey };
    expect((await req("/auth/device/exchange", { ...b, verifier: "x".repeat(43) })).status).toBe(
      401,
    );
    expect((await req("/auth/device/exchange", b)).status).toBe(200);
    expect((await req("/auth/device/exchange", b)).status).toBe(401);
    const cloned = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
      "sign",
      "verify",
    ]);
    const clonedKey = Buffer.from(await crypto.subtle.exportKey("raw", cloned.publicKey)).toString(
      "base64url",
    );
    const newCode = "d".repeat(43);
    await db
      .prepare("INSERT INTO authorization_codes VALUES(?,?,?,?,9999999999)")
      .bind(await hash(newCode), user, await hash(verifier), state)
      .run();
    expect(
      (await req("/auth/device/exchange", { ...b, code: newCode, recovery_key: clonedKey })).status,
    ).toBe(409);
  });
  it("recovery is single-use and stores hashes; management requires step-up", async () => {
    const code = "z".repeat(43);
    await db
      .prepare("INSERT INTO recovery_codes VALUES(?,?,?,?,NULL)")
      .bind("recovery", user, await hash(code), 0)
      .run();
    const b = { username: "alice", code };
    expect((await req("/auth/recover", b)).status).toBe(200);
    expect((await req("/auth/recover", b)).status).toBe(401);
    await db.prepare("UPDATE device_sessions SET stepup_until=0").run();
    expect((await req("/data/reset", {})).status).toBe(403);
  });
  it("cannot register device ID owned by another account or reuse revoked IDs", async () => {
    const other = crypto.randomUUID();
    await db.prepare("INSERT INTO users VALUES(?,?,0,0)").bind(other, "bob").run();
    const code = "a".repeat(43),
      verifier = "b".repeat(43),
      state = "c".repeat(43);
    await db
      .prepare("INSERT INTO authorization_codes VALUES(?,?,?,?,9999999999)")
      .bind(await hash(code), other, await hash(verifier), state)
      .run();
    expect(
      (
        await req("/auth/device/exchange", {
          code,
          verifier,
          state,
          device_id: device,
          name: "cloned",
          recovery_key: recoveryKey,
        })
      ).status,
    ).toBe(409);
  });
  it("rejects cross-origin, unauthenticated and oversized uploads", async () => {
    expect((await req("/stats", undefined, "GET", "wrong")).status).toBe(401);
    expect(
      (
        await app.request(
          "http://localhost/api/v1/auth/login/options",
          { method: "POST", headers: { origin: "https://evil.test" } },
          env,
        )
      ).status,
    ).toBe(403);
    expect((await req("/sync/push", { text: "x".repeat(100001) })).status).toBe(413);
  });
  it("can retry a lost refresh response with the same nonce after restart", async () => {
    const refresh = "R".repeat(43),
      nonce = "n".repeat(43);
    await db
      .prepare("UPDATE device_sessions SET refresh_hash=?")
      .bind(await hash(refresh))
      .run();
    const first = await (await req("/auth/refresh", { refresh_token: refresh, nonce })).json();
    const retry = await (await req("/auth/refresh", { refresh_token: refresh, nonce })).json();
    expect(retry.access_token).toBe(first.access_token);
    expect(retry.refresh_token).toBe(first.refresh_token);
    const next = await req("/auth/refresh", {
      refresh_token: first.refresh_token,
      nonce: "x".repeat(43),
    });
    expect(next.status).toBe(200);
    expect((await req("/auth/refresh", { refresh_token: refresh, nonce })).status).toBe(401);
  });
  it("trusted device recovery requires the separate private key, single-use proof, and live session", async () => {
    const options = await (await req("/device-recovery/options", {})).json();
    const signature = Buffer.from(
      await crypto.subtle.sign(
        { name: "ECDSA", hash: "SHA-256" },
        identity.privateKey,
        new TextEncoder().encode(options.challenge),
      ),
    ).toString("base64url");
    const proof = { challenge_id: options.challenge_id, signature };
    const verified = await req("/device-recovery/verify", proof);
    expect(verified.status).toBe(200);
    expect((await req("/device-recovery/verify", proof)).status).toBe(401);
    const { code } = await verified.json();
    expect((await req("/auth/device-recovery/exchange", { code })).status).toBe(200);
    expect((await req("/auth/device-recovery/exchange", { code })).status).toBe(401);
    const ch = await (await req("/device-recovery/options", {})).json();
    expect(
      (
        await req("/device-recovery/verify", {
          challenge_id: ch.challenge_id,
          signature: "x".repeat(86),
        })
      ).status,
    ).toBe(401);
  });
  it("maximum-sized upload stays below the Free D1 invocation query limit", async () => {
    const b = { generation: 0, recordings: [], rows: [] } as {
      generation: number;
      recordings: ReturnType<typeof batch>["recordings"];
      rows: ReturnType<typeof batch>["rows"];
    };
    for (let i = 0; i < 20; i++) {
      const part = batch(i + 1);
      const id = crypto.randomUUID();
      part.recordings[0]!.id = id;
      part.rows[0]!.recording_id = id;
      b.recordings.push(...part.recordings);
      b.rows.push(...part.rows);
    }
    const before = db.queryCount();
    expect((await req("/sync/push", b)).status).toBe(200);
    expect(db.queryCount() - before).toBeLessThanOrEqual(50);
  });
  it("keyset pagination restores every row once and honors IANA day filters", async () => {
    await req("/sync/push", batch());
    for (let i = 1; i <= 1005; i++)
      await db
        .prepare("INSERT INTO hourly VALUES(?,?,30,1)")
        .bind(recording, i * 3600)
        .run();
    const first = await (await req("/sync/pull")).json();
    expect(first.rows).toHaveLength(1000);
    expect(typeof first.next_cursor).toBe("string");
    const last = await (await req(`/sync/pull?cursor=${first.next_cursor}`)).json();
    expect(last.rows).toHaveLength(6);
    expect(last.next_cursor).toBeNull();
    expect(
      new Set(
        [...first.rows, ...last.rows].map((r) => `${r.recording_id}:${r.hour_start}:${r.key_code}`),
      ).size,
    ).toBe(1006);
    const filtered = await (
      await req("/stats?from=1970-01-02&to=1970-01-02&timezone=Asia%2FTokyo")
    ).json();
    expect(filtered.rows).toHaveLength(24);
    expect((await req("/stats?cursor=bad")).status).toBe(400);
    expect((await req("/stats?timezone=invalid")).status).toBe(400);
  });
});
