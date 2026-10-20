import { Hono, type Context } from "hono";
import { HTTPException } from "hono/http-exception";
import { z } from "zod";
import {
  generateRegistrationOptions,
  verifyRegistrationResponse,
  generateAuthenticationOptions,
  verifyAuthenticationResponse,
} from "@simplewebauthn/server";
import type {
  RegistrationResponseJSON,
  AuthenticationResponseJSON,
  AuthenticatorTransportFuture,
} from "@simplewebauthn/server";
import { pushSchema, localDay } from "@mouse-stats/contracts";

export interface Env {
  DB: D1Database;
  ORIGIN: string;
  RP_ID: string;
}
export async function cleanupExpired(db: D1Database, timestamp = Math.floor(Date.now() / 1000)) {
  await db.batch([
    db.prepare("DELETE FROM challenges WHERE expires_at<=?").bind(timestamp),
    db.prepare("DELETE FROM rate_limits WHERE expires_at<=?").bind(timestamp),
  ]);
}
type User = { id: string; username: string; data_generation: number };
type Session = {
  id: string;
  user_id: string;
  device_id: string | null;
  stepup_until: number;
  data_generation: number;
};
type Passkey = {
  credential_id: string;
  user_id: string;
  public_key: string;
  counter: number;
  transports: string;
  name: string;
};
type AuthContext = {
  username: string;
  state: string;
  pkce: string;
  mode: "device" | "manage";
  user_id?: string;
};
const now = () => Math.floor(Date.now() / 1000);
const token = () => b64(crypto.getRandomValues(new Uint8Array(32)));
const b64 = (bytes: Uint8Array) =>
  btoa(String.fromCharCode(...bytes))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
const unb64 = (s: string) =>
  Uint8Array.from(atob(s.replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
export const hash = async (s: string) =>
  b64(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s))));
async function derive(secret: string, nonce: string, label: string) {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return b64(
    new Uint8Array(
      await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${label}:${nonce}`)),
    ),
  );
}
const fail = (status: 400 | 401 | 403 | 404 | 409 | 413 | 429, message: string): never => {
  throw new HTTPException(status, { message });
};
const username = z
  .string()
  .min(3)
  .max(32)
  .regex(/^[a-zA-Z0-9_-]+$/)
  .transform((s) => s.toLowerCase());
const optionsSchema = z
  .object({
    username,
    state: z.string().regex(/^[A-Za-z0-9_-]{32,128}$/),
    pkce: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
    mode: z.enum(["device", "manage"]).default("device"),
  })
  .strict();
const requestBodies = new WeakMap<Request, string>();
async function readBody(request: Request) {
  const reader = request.body?.getReader();
  if (!reader) return "";
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > 100_000) {
      await reader.cancel();
      fail(413, "BODY_TOO_LARGE");
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return new TextDecoder().decode(bytes);
}
const json = async <T extends z.ZodType>(request: Request, schema: T): Promise<z.infer<T>> => {
  if (!request.headers.get("content-type")?.startsWith("application/json"))
    fail(400, "JSON_REQUIRED");
  const raw = requestBodies.get(request) ?? (await readBody(request));
  if (raw.length > 100_000) fail(413, "BODY_TOO_LARGE");
  try {
    return schema.parse(JSON.parse(raw));
  } catch {
    return fail(400, "INVALID_REQUEST");
  }
};
const get = async <T>(db: D1Database, sql: string, ...args: unknown[]) =>
  db
    .prepare(sql)
    .bind(...args)
    .first<T>();
const all = async <T>(db: D1Database, sql: string, ...args: unknown[]) =>
  (
    await db
      .prepare(sql)
      .bind(...args)
      .all<T>()
  ).results;
const run = async (db: D1Database, sql: string, ...args: unknown[]) =>
  db
    .prepare(sql)
    .bind(...args)
    .run();
async function issue(db: D1Database, user_id: string, device_id: string | null, admin = false) {
  const access_token = token(),
    refresh_token = device_id && !admin ? token() : null;
  await run(
    db,
    "INSERT INTO device_sessions(id,user_id,device_id,refresh_hash,access_hash,access_expires,expires_at,stepup_until) VALUES(?,?,?,?,?,?,?,?)",
    crypto.randomUUID(),
    user_id,
    device_id,
    refresh_token ? await hash(refresh_token) : null,
    await hash(access_token),
    now() + 300,
    now() + (device_id && !admin ? 30 * 86400 : 300),
    admin ? now() + 300 : 0,
  );
  return { access_token, refresh_token, expires_in: 300 };
}
async function issueBrowserSession(
  c: Context<{ Bindings: Env; Variables: { session: Session } }>,
  db: D1Database,
  user_id: string,
) {
  const shortSession = await issue(db, user_id, null, true);
  const cookieToken = token();
  const lifetime = 30 * 86400;
  await run(
    db,
    "INSERT INTO device_sessions(id,user_id,device_id,refresh_hash,access_hash,access_expires,expires_at,stepup_until) VALUES(?,?,NULL,NULL,?,?,?,0)",
    crypto.randomUUID(),
    user_id,
    await hash(cookieToken),
    now() + lifetime,
    now() + lifetime,
  );
  const secure = c.env.ORIGIN.startsWith("https://") ? "; Secure" : "";
  c.header(
    "Set-Cookie",
    `mouse_stats_session=${cookieToken}; Path=/; HttpOnly; SameSite=Strict; Max-Age=${lifetime}${secure}`,
  );
  return shortSession;
}
function browserCookie(request: Request) {
  return request.headers
    .get("cookie")
    ?.split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith("mouse_stats_session="))
    ?.slice("mouse_stats_session=".length);
}
export function createApi() {
  const app = new Hono<{ Bindings: Env; Variables: { session: Session } }>();
  app.onError((e, c) => {
    if (e instanceof HTTPException) return c.json({ error: e.message }, e.status);
    // Classify database failures without logging SQL, request data or secrets.
    const reason =
      e.message.match(
        /(?:UNIQUE|NOT NULL|FOREIGN KEY|CHECK) constraint failed(?:: [a-zA-Z0-9_. ,]+)?|SQLITE_[A-Z_]+|D1_[A-Z_]+/,
      )?.[0] ?? e.name;
    console.error("API_INTERNAL_ERROR", reason);
    return c.json({ error: "INTERNAL_ERROR" }, 500);
  });
  app.use("*", async (c, next) => {
    c.header("Cache-Control", "no-store");
    c.header("X-Content-Type-Options", "nosniff");
    if (c.req.method !== "GET" && c.req.header("origin") && c.req.header("origin") !== c.env.ORIGIN)
      fail(403, "ORIGIN_MISMATCH");
    const length = Number(c.req.header("content-length") ?? 0);
    if (length > 100_000) fail(413, "BODY_TOO_LARGE");
    if (c.req.method !== "GET" && c.req.method !== "HEAD")
      requestBodies.set(c.req.raw, await readBody(c.req.raw));
    const slot = Math.floor(now() / 60),
      ip = c.req.header("cf-connecting-ip") ?? "local";
    const key = await hash(
      `${ip}:${slot}:${c.req.path.startsWith("/api/v1/auth") ? "auth" : "api"}`,
    );
    const limit = await get<{ count: number }>(
      c.env.DB,
      "INSERT INTO rate_limits(key,count,expires_at) VALUES(?,1,?) ON CONFLICT(key) DO UPDATE SET count=count+1 RETURNING count",
      key,
      now() + 120,
    );
    if ((limit?.count ?? 0) > (c.req.path.startsWith("/api/v1/auth") ? 60 : 240))
      fail(429, "RATE_LIMIT");
    await next();
  });
  app.get("/api/health", (c) => c.json({ ok: true }));
  for (const purpose of ["register", "login"] as const) {
    app.post(`/api/v1/auth/${purpose}/options`, async (c) => {
      const ctx = await json(c.req.raw, optionsSchema);
      const user = await get<User>(c.env.DB, "SELECT * FROM users WHERE username=?", ctx.username);
      if (purpose === "register" && user) fail(409, "USERNAME_UNAVAILABLE");
      if (purpose === "login" && !user) fail(401, "AUTHENTICATION_FAILED");
      const userId = user?.id ?? crypto.randomUUID();
      const keys = user
        ? await all<Passkey>(
            c.env.DB,
            "SELECT * FROM passkeys WHERE user_id=? AND revoked_at IS NULL",
            user.id,
          )
        : [];
      const options =
        purpose === "register"
          ? await generateRegistrationOptions({
              rpName: "Mouse Stats",
              rpID: c.env.RP_ID,
              userName: ctx.username,
              userID: new TextEncoder().encode(userId),
              attestationType: "none",
              authenticatorSelection: { residentKey: "required", userVerification: "required" },
            })
          : await generateAuthenticationOptions({
              rpID: c.env.RP_ID,
              userVerification: "required",
              allowCredentials: keys.map((k) => ({
                id: k.credential_id,
                transports: JSON.parse(k.transports),
              })),
            });
      const challenge_id = crypto.randomUUID();
      await run(
        c.env.DB,
        "INSERT INTO challenges VALUES(?,?,?,?,?)",
        challenge_id,
        options.challenge,
        purpose,
        JSON.stringify({ ...ctx, user_id: userId }),
        now() + 300,
      );
      await cleanupExpired(c.env.DB, now());
      return c.json({ challenge_id, options });
    });
    app.post(`/api/v1/auth/${purpose}/verify`, async (c) => {
      const input = await json(
        c.req.raw,
        z.object({ challenge_id: z.uuid(), response: z.record(z.string(), z.unknown()) }),
      );
      const challenge = await get<{ challenge: string; context: string }>(
        c.env.DB,
        "DELETE FROM challenges WHERE id=? AND purpose=? AND expires_at>? RETURNING challenge,context",
        input.challenge_id,
        purpose,
        now(),
      );
      if (!challenge) fail(401, "CHALLENGE_EXPIRED");
      const ctx = JSON.parse(challenge!.context) as AuthContext;
      let user_id = ctx.user_id;
      try {
        if (purpose === "register") {
          const result = await verifyRegistrationResponse({
            response: input.response as unknown as RegistrationResponseJSON,
            expectedChallenge: challenge!.challenge,
            expectedOrigin: c.env.ORIGIN,
            expectedRPID: c.env.RP_ID,
            requireUserVerification: true,
          });
          if (!result.verified || !result.registrationInfo) fail(401, "AUTHENTICATION_FAILED");
          user_id = ctx.user_id!;
          const cred = result.registrationInfo!.credential;
          await c.env.DB.batch([
            c.env.DB.prepare("INSERT INTO users VALUES(?,?,0,?)").bind(
              user_id,
              ctx.username,
              now(),
            ),
            c.env.DB.prepare(
              "INSERT INTO passkeys(credential_id,user_id,public_key,counter,transports,name,created_at,last_used) VALUES(?,?,?,?,?,?,?,?)",
            ).bind(
              cred.id,
              user_id,
              b64(cred.publicKey),
              cred.counter,
              JSON.stringify(cred.transports ?? []),
              "最初のパスキー",
              now(),
              now(),
            ),
          ]);
        } else {
          const cred = await get<Passkey>(
            c.env.DB,
            "SELECT * FROM passkeys WHERE credential_id=? AND user_id=? AND revoked_at IS NULL",
            String(input.response.id),
            user_id,
          );
          if (!cred) fail(401, "AUTHENTICATION_FAILED");
          const result = await verifyAuthenticationResponse({
            response: input.response as unknown as AuthenticationResponseJSON,
            expectedChallenge: challenge!.challenge,
            expectedOrigin: c.env.ORIGIN,
            expectedRPID: c.env.RP_ID,
            requireUserVerification: true,
            credential: {
              id: cred!.credential_id,
              publicKey: unb64(cred!.public_key),
              counter: cred!.counter,
              transports: JSON.parse(cred!.transports) as AuthenticatorTransportFuture[],
            },
          });
          if (!result.verified) fail(401, "AUTHENTICATION_FAILED");
          // Counter 0 is valid for synced passkeys; nonzero counters use compare-and-swap.
          const changed = await run(
            c.env.DB,
            "UPDATE passkeys SET counter=?,last_used=? WHERE credential_id=? AND counter=? AND revoked_at IS NULL",
            result.authenticationInfo.newCounter,
            now(),
            cred!.credential_id,
            cred!.counter,
          );
          if (changed.meta.changes !== 1) fail(401, "AUTHENTICATION_REPLAY");
        }
      } catch (e) {
        if (e instanceof HTTPException) throw e;
        return fail(401, "AUTHENTICATION_FAILED");
      }
      if (ctx.mode === "manage")
        return c.json({ ...(await issueBrowserSession(c, c.env.DB, user_id!)), state: ctx.state });
      const code = token();
      await run(
        c.env.DB,
        "INSERT INTO authorization_codes VALUES(?,?,?,?,?)",
        await hash(code),
        user_id,
        ctx.pkce,
        ctx.state,
        now() + 60,
      );
      return c.json({
        code,
        state: ctx.state,
        redirect: `mousestats://auth?code=${code}&state=${ctx.state}`,
      });
    });
  }
  app.post("/api/v1/auth/device/exchange", async (c) => {
    const b = await json(
      c.req.raw,
      z.object({
        code: z.string().min(40).max(100),
        state: z.string().min(32).max(128),
        verifier: z.string().regex(/^[A-Za-z0-9_-]{43,128}$/),
        device_id: z.uuid(),
        name: z.string().min(1).max(80),
        recovery_key: z.string().regex(/^[A-Za-z0-9_-]{87}$/),
      }),
    );
    const code = await get<{ user_id: string; state: string }>(
      c.env.DB,
      "DELETE FROM authorization_codes WHERE code_hash=? AND pkce=? AND state=? AND expires_at>? RETURNING user_id",
      await hash(b.code),
      await hash(b.verifier),
      b.state,
      now(),
    );
    if (!code) fail(401, "INVALID_CODE");
    try {
      await crypto.subtle.importKey(
        "raw",
        unb64(b.recovery_key),
        { name: "ECDSA", namedCurve: "P-256" },
        false,
        ["verify"],
      );
    } catch {
      return fail(400, "INVALID_RECOVERY_KEY");
    }
    const device = await get<{ user_id: string; revoked_at: number | null; recovery_key: string }>(
      c.env.DB,
      "SELECT user_id,revoked_at,recovery_key FROM devices WHERE id=?",
      b.device_id,
    );
    if (
      device &&
      (device.user_id !== code!.user_id ||
        device.revoked_at !== null ||
        (device.recovery_key !== "" && device.recovery_key !== b.recovery_key))
    )
      fail(409, "DEVICE_ID_CONFLICT");
    await run(
      c.env.DB,
      "INSERT INTO devices(id,user_id,name,created_at,last_seen,revoked_at,recovery_key) VALUES(?,?,?,?,?,NULL,?) ON CONFLICT(id) DO UPDATE SET last_seen=excluded.last_seen,name=excluded.name,recovery_key=excluded.recovery_key",
      b.device_id,
      code!.user_id,
      b.name,
      now(),
      now(),
      b.recovery_key,
    );
    const user = await get<User>(c.env.DB, "SELECT * FROM users WHERE id=?", code!.user_id);
    return c.json({
      ...(await issue(c.env.DB, code!.user_id, b.device_id)),
      user_id: code!.user_id,
      generation: user!.data_generation,
    });
  });
  app.post("/api/v1/auth/refresh", async (c) => {
    const b = await json(
      c.req.raw,
      z.object({
        refresh_token: z.string().min(40).max(100),
        nonce: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
      }),
    );
    const access_token = await derive(b.refresh_token, b.nonce, "access"),
      refresh_token = await derive(b.refresh_token, b.nonce, "refresh");
    const oldHash = await hash(b.refresh_token),
      nonceHash = await hash(b.nonce),
      newHash = await hash(refresh_token);
    let session = await get<{ user_id: string; device_id: string }>(
      c.env.DB,
      `UPDATE device_sessions SET previous_refresh_hash=refresh_hash,rotation_nonce_hash=?,refresh_hash=?,access_hash=?,access_expires=? WHERE refresh_hash=? AND expires_at>? AND revoked_at IS NULL AND device_id IN(SELECT id FROM devices WHERE revoked_at IS NULL) RETURNING user_id,device_id`,
      nonceHash,
      newHash,
      await hash(access_token),
      now() + 300,
      oldHash,
      now(),
    );
    // The same persisted nonce retries a response lost in transit. A different nonce cannot replay the old token.
    if (!session)
      session = await get(
        c.env.DB,
        "UPDATE device_sessions SET access_expires=? WHERE previous_refresh_hash=? AND rotation_nonce_hash=? AND refresh_hash=? AND expires_at>? AND revoked_at IS NULL AND device_id IN(SELECT id FROM devices WHERE revoked_at IS NULL) RETURNING user_id,device_id",
        now() + 300,
        oldHash,
        nonceHash,
        newHash,
        now(),
      );
    if (!session) fail(401, "SESSION_REVOKED");
    const user = await get<User>(c.env.DB, "SELECT * FROM users WHERE id=?", session!.user_id);
    return c.json({
      access_token,
      refresh_token,
      expires_in: 300,
      user_id: session!.user_id,
      generation: user!.data_generation,
    });
  });
  app.post("/api/v1/auth/device/logout", async (c) => {
    const b = await json(c.req.raw, z.object({ refresh_token: z.string().min(40).max(100) }));
    const digest = await hash(b.refresh_token);
    await run(
      c.env.DB,
      "UPDATE device_sessions SET revoked_at=? WHERE refresh_hash=? OR previous_refresh_hash=?",
      now(),
      digest,
      digest,
    );
    return c.json({ ok: true });
  });
  app.post("/api/v1/auth/device-recovery/exchange", async (c) => {
    const b = await json(c.req.raw, z.object({ code: z.string().min(40).max(100) }));
    const code = await get<{ user_id: string; state: string }>(
      c.env.DB,
      "DELETE FROM authorization_codes WHERE code_hash=? AND pkce='device-recovery' AND expires_at>? RETURNING user_id,state",
      await hash(b.code),
      now(),
    );
    if (!code) fail(401, "INVALID_CODE");
    const device = await get<{ device_id: string }>(
      c.env.DB,
      "SELECT s.device_id FROM device_sessions s JOIN devices d ON d.id=s.device_id WHERE s.id=? AND s.user_id=? AND s.revoked_at IS NULL AND d.revoked_at IS NULL AND s.expires_at>?",
      code!.state,
      code!.user_id,
      now(),
    );
    if (!device) fail(401, "DEVICE_REVOKED");
    return c.json(await issueBrowserSession(c, c.env.DB, code!.user_id));
  });
  // Recovery codes are independent 256-bit, single-use proofs. No email fallback.
  app.post("/api/v1/auth/recover", async (c) => {
    const b = await json(c.req.raw, z.object({ username, code: z.string().min(40).max(100) }));
    const owner = await get<User>(c.env.DB, "SELECT * FROM users WHERE username=?", b.username);
    if (!owner) fail(401, "RECOVERY_FAILED");
    const proof = await get<{ user_id: string }>(
      c.env.DB,
      "UPDATE recovery_codes SET used_at=? WHERE code_hash=? AND used_at IS NULL AND user_id=? RETURNING user_id",
      now(),
      await hash(b.code),
      owner!.id,
    );
    if (!proof) fail(401, "RECOVERY_FAILED");
    return c.json(await issueBrowserSession(c, c.env.DB, proof!.user_id));
  });
  app.use("/api/v1/*", async (c, next) => {
    const authorization = c.req.header("authorization");
    const credential = authorization
      ? authorization.startsWith("Bearer ")
        ? authorization.slice(7)
        : ""
      : browserCookie(c.req.raw);
    if (!credential) fail(401, "AUTHENTICATION_REQUIRED");
    const s = await get<Session>(
      c.env.DB,
      `SELECT s.id,s.user_id,s.device_id,s.stepup_until,u.data_generation FROM device_sessions s JOIN users u ON u.id=s.user_id LEFT JOIN devices d ON d.id=s.device_id WHERE s.access_hash=? AND s.access_expires>? AND s.expires_at>? AND s.revoked_at IS NULL AND (s.device_id IS NULL OR d.revoked_at IS NULL)`,
      await hash(credential!),
      now(),
      now(),
    );
    if (!s) fail(401, "SESSION_EXPIRED");
    c.set("session", s!);
    await next();
  });
  app.get("/api/v1/auth/session", (c) => c.json({ signed_in: true }));
  const admin = (s: Session) => {
    if (s.stepup_until <= now()) fail(403, "PASSKEY_REAUTHENTICATION_REQUIRED");
  };
  app.post("/api/v1/device-recovery/options", async (c) => {
    const s = c.get("session");
    if (!s.device_id) fail(403, "DEVICE_REQUIRED");
    const device = await get<{ recovery_key: string }>(
      c.env.DB,
      "SELECT recovery_key FROM devices WHERE id=? AND revoked_at IS NULL",
      s.device_id,
    );
    if (!device?.recovery_key) fail(409, "DEVICE_RECOVERY_UNAVAILABLE");
    const challenge_id = crypto.randomUUID(),
      challenge = token();
    await run(
      c.env.DB,
      "INSERT INTO challenges VALUES(?,?,?,?,?)",
      challenge_id,
      challenge,
      "device-recovery",
      JSON.stringify({ user_id: s.user_id, device_id: s.device_id, session_id: s.id }),
      now() + 120,
    );
    return c.json({ challenge_id, challenge });
  });
  app.post("/api/v1/device-recovery/verify", async (c) => {
    const s = c.get("session");
    if (!s.device_id) fail(403, "DEVICE_REQUIRED");
    const b = await json(
      c.req.raw,
      z.object({ challenge_id: z.uuid(), signature: z.string().regex(/^[A-Za-z0-9_-]{86}$/) }),
    );
    const ch = await get<{ challenge: string }>(
      c.env.DB,
      "DELETE FROM challenges WHERE id=? AND purpose='device-recovery' AND json_extract(context,'$.session_id')=? AND expires_at>? RETURNING challenge",
      b.challenge_id,
      s.id,
      now(),
    );
    if (!ch) fail(401, "CHALLENGE_EXPIRED");
    const device = await get<{ recovery_key: string }>(
      c.env.DB,
      "SELECT recovery_key FROM devices WHERE id=? AND user_id=? AND revoked_at IS NULL",
      s.device_id,
      s.user_id,
    );
    if (!device?.recovery_key) fail(401, "DEVICE_REVOKED");
    try {
      const key = await crypto.subtle.importKey(
        "raw",
        unb64(device!.recovery_key),
        { name: "ECDSA", namedCurve: "P-256" },
        false,
        ["verify"],
      );
      const verified = await crypto.subtle.verify(
        { name: "ECDSA", hash: "SHA-256" },
        key,
        unb64(b.signature),
        new TextEncoder().encode(ch!.challenge),
      );
      if (!verified) fail(401, "DEVICE_PROOF_REQUIRED");
    } catch {
      return fail(401, "DEVICE_PROOF_REQUIRED");
    }
    const code = token();
    await run(
      c.env.DB,
      "INSERT INTO authorization_codes VALUES(?,?,?,?,?)",
      await hash(code),
      s.user_id,
      "device-recovery",
      s.id,
      now() + 60,
    );
    return c.json({ code });
  });
  app.post("/api/v1/auth/logout", async (c) => {
    await run(
      c.env.DB,
      "UPDATE device_sessions SET revoked_at=? WHERE user_id=? AND device_id IS NULL AND revoked_at IS NULL",
      now(),
      c.get("session").user_id,
    );
    const secure = c.env.ORIGIN.startsWith("https://") ? "; Secure" : "";
    c.header(
      "Set-Cookie",
      `mouse_stats_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0${secure}`,
    );
    return c.json({ ok: true });
  });
  app.get("/api/v1/devices", async (c) =>
    c.json({
      devices: await all(
        c.env.DB,
        "SELECT id,name,last_seen,revoked_at FROM devices WHERE user_id=? ORDER BY created_at",
        c.get("session").user_id,
      ),
    }),
  );
  app.post("/api/v1/devices/:id/revoke", async (c) => {
    const s = c.get("session");
    admin(s);
    await c.env.DB.batch([
      c.env.DB.prepare("UPDATE devices SET revoked_at=? WHERE id=? AND user_id=?").bind(
        now(),
        c.req.param("id"),
        s.user_id,
      ),
      c.env.DB.prepare(
        "UPDATE device_sessions SET revoked_at=? WHERE device_id=? AND user_id=?",
      ).bind(now(), c.req.param("id"), s.user_id),
    ]);
    return c.json({ ok: true });
  });
  app.get("/api/v1/passkeys", async (c) => {
    const s = c.get("session");
    admin(s);
    return c.json({
      passkeys: await all(
        c.env.DB,
        "SELECT credential_id,name,created_at,last_used,revoked_at FROM passkeys WHERE user_id=?",
        s.user_id,
      ),
    });
  });
  app.post("/api/v1/passkeys/options", async (c) => {
    const s = c.get("session");
    admin(s);
    const user = await get<User>(c.env.DB, "SELECT * FROM users WHERE id=?", s.user_id);
    const keys = await all<Passkey>(
      c.env.DB,
      "SELECT * FROM passkeys WHERE user_id=? AND revoked_at IS NULL",
      s.user_id,
    );
    const options = await generateRegistrationOptions({
      rpName: "Mouse Stats",
      rpID: c.env.RP_ID,
      userName: user!.username,
      userID: new TextEncoder().encode(s.user_id),
      attestationType: "none",
      authenticatorSelection: { residentKey: "required", userVerification: "required" },
      excludeCredentials: keys.map((k) => ({ id: k.credential_id })),
    });
    const challenge_id = crypto.randomUUID();
    await run(
      c.env.DB,
      "INSERT INTO challenges VALUES(?,?,?,?,?)",
      challenge_id,
      options.challenge,
      "add",
      JSON.stringify({ user_id: s.user_id }),
      now() + 300,
    );
    return c.json({ challenge_id, options });
  });
  app.post("/api/v1/passkeys", async (c) => {
    const s = c.get("session");
    admin(s);
    const b = await json(
      c.req.raw,
      z.object({
        challenge_id: z.uuid(),
        name: z.string().min(1).max(80),
        response: z.record(z.string(), z.unknown()),
      }),
    );
    const ch = await get<{ challenge: string }>(
      c.env.DB,
      `DELETE FROM challenges WHERE id=? AND purpose='add' AND json_extract(context,'$.user_id')=? AND expires_at>? RETURNING challenge`,
      b.challenge_id,
      s.user_id,
      now(),
    );
    if (!ch) fail(401, "CHALLENGE_EXPIRED");
    try {
      const v = await verifyRegistrationResponse({
        response: b.response as unknown as RegistrationResponseJSON,
        expectedChallenge: ch!.challenge,
        expectedOrigin: c.env.ORIGIN,
        expectedRPID: c.env.RP_ID,
        requireUserVerification: true,
      });
      if (!v.verified || !v.registrationInfo) fail(401, "AUTHENTICATION_FAILED");
      const k = v.registrationInfo!.credential;
      await run(
        c.env.DB,
        "INSERT INTO passkeys(credential_id,user_id,public_key,counter,transports,name,created_at) VALUES(?,?,?,?,?,?,?)",
        k.id,
        s.user_id,
        b64(k.publicKey),
        k.counter,
        JSON.stringify(k.transports ?? []),
        b.name,
        now(),
      );
      return c.json({ ok: true });
    } catch {
      return fail(401, "AUTHENTICATION_FAILED");
    }
  });
  app.patch("/api/v1/passkeys/:id", async (c) => {
    const s = c.get("session");
    admin(s);
    const b = await json(c.req.raw, z.object({ name: z.string().min(1).max(80) }));
    await run(
      c.env.DB,
      "UPDATE passkeys SET name=? WHERE credential_id=? AND user_id=?",
      b.name,
      c.req.param("id"),
      s.user_id,
    );
    return c.json({ ok: true });
  });
  app.delete("/api/v1/passkeys/:id", async (c) => {
    const s = c.get("session");
    admin(s);
    await run(
      c.env.DB,
      "UPDATE passkeys SET revoked_at=? WHERE credential_id=? AND user_id=? AND (SELECT COUNT(*) FROM passkeys WHERE user_id=? AND revoked_at IS NULL)>1",
      now(),
      c.req.param("id"),
      s.user_id,
      s.user_id,
    );
    return c.json({ ok: true });
  });
  app.post("/api/v1/recovery", async (c) => {
    const s = c.get("session");
    admin(s);
    const codes = Array.from({ length: 8 }, token);
    const statements = [
      c.env.DB.prepare("DELETE FROM recovery_codes WHERE user_id=?").bind(s.user_id),
    ];
    for (const code of codes)
      statements.push(
        c.env.DB.prepare("INSERT INTO recovery_codes VALUES(?,?,?,?,NULL)").bind(
          crypto.randomUUID(),
          s.user_id,
          await hash(code),
          now(),
        ),
      );
    await c.env.DB.batch(statements);
    return c.json({ codes });
  });
  app.post("/api/v1/data/reset", async (c) => {
    const s = c.get("session");
    admin(s);
    await c.env.DB.batch([
      c.env.DB.prepare("UPDATE users SET data_generation=data_generation+1 WHERE id=?").bind(
        s.user_id,
      ),
      c.env.DB.prepare(
        "DELETE FROM recordings WHERE device_id IN(SELECT id FROM devices WHERE user_id=?)",
      ).bind(s.user_id),
    ]);
    const user = await get<User>(c.env.DB, "SELECT * FROM users WHERE id=?", s.user_id);
    return c.json({ generation: user!.data_generation });
  });
  app.delete("/api/v1/account", async (c) => {
    const s = c.get("session");
    admin(s);
    await run(c.env.DB, "DELETE FROM users WHERE id=?", s.user_id);
    return c.json({ ok: true });
  });
  app.post("/api/v1/sync/push", async (c) => {
    const s = c.get("session");
    if (!s.device_id) fail(403, "DEVICE_REQUIRED");
    const b = await json(c.req.raw, pushSchema);
    if (b.generation !== s.data_generation) fail(409, "GENERATION_CHANGED");
    const ids = new Set(b.recordings.map((r) => r.id));
    for (const row of b.rows) {
      if (!ids.has(row.recording_id)) fail(400, "RECORDING_REQUIRED");
      if (row.hour_start > now() + 3600) fail(400, "FUTURE_HOUR");
    }
    const existing = b.recordings.length
      ? await all<{ id: string; device_id: string }>(
          c.env.DB,
          `SELECT id,device_id FROM recordings WHERE id IN (${b.recordings.map(() => "?").join(",")})`,
          ...b.recordings.map((r) => r.id),
        )
      : [];
    const owners = new Map(existing.map((r) => [r.id, r.device_id]));
    for (const r of b.recordings) {
      if (r.data_generation !== s.data_generation) fail(409, "GENERATION_CHANGED");
      const owner = owners.get(r.id);
      if (owner && owner !== s.device_id) fail(403, "RECORDING_OWNERSHIP");
    }
    // Every write rechecks generation, session and device in SQL: concurrent reset/revoke cannot resurrect data.
    const guard = `EXISTS(SELECT 1 FROM users u JOIN devices d ON d.user_id=u.id JOIN device_sessions s ON s.device_id=d.id WHERE u.id=? AND u.data_generation=? AND d.id=? AND d.revoked_at IS NULL AND s.id=? AND s.revoked_at IS NULL)`;
    const args = [s.user_id, b.generation, s.device_id, s.id];
    const statements = b.recordings.map((r) =>
      c.env.DB.prepare(
        `INSERT INTO recordings SELECT ?,?,?,?,?,? WHERE ${guard} ON CONFLICT(id) DO UPDATE SET device_id=CASE WHEN recordings.device_id=excluded.device_id THEN recordings.device_id ELSE NULL END,ended_at=COALESCE(excluded.ended_at,recordings.ended_at)`,
      ).bind(
        r.id,
        s.device_id,
        r.data_generation,
        r.started_at,
        r.ended_at,
        r.key_schema_version,
        ...args,
      ),
    );
    for (const r of b.rows)
      statements.push(
        c.env.DB.prepare(
          `INSERT INTO hourly SELECT ?,?,?,? WHERE ${guard} AND EXISTS(SELECT 1 FROM recordings WHERE id=? AND device_id=? AND data_generation=?) ON CONFLICT(recording_id,hour_start,key_code) DO UPDATE SET value=MAX(hourly.value,excluded.value)`,
        ).bind(
          r.recording_id,
          r.hour_start,
          r.key_code ?? -1,
          r.value,
          ...args,
          r.recording_id,
          s.device_id,
          b.generation,
        ),
      );
    statements.push(
      c.env.DB.prepare("UPDATE devices SET last_seen=? WHERE id=?").bind(now(), s.device_id),
    );
    await c.env.DB.batch(statements);
    const current = await get<User>(c.env.DB, "SELECT * FROM users WHERE id=?", s.user_id);
    if (current?.data_generation !== b.generation) fail(409, "GENERATION_CHANGED");
    const live = await get(
      c.env.DB,
      "SELECT id FROM devices WHERE id=? AND revoked_at IS NULL",
      s.device_id,
    );
    if (!live) fail(401, "DEVICE_REVOKED");
    return c.json({
      acked: b.rows.map((r) => ({
        recording_id: r.recording_id,
        hour_start: r.hour_start,
        key_code: r.key_code,
        revision: r.revision,
      })),
      generation: b.generation,
    });
  });
  for (const path of ["/api/v1/sync/pull", "/api/v1/stats"])
    app.get(path, async (c) => {
      const s = c.get("session");
      const pull = path.endsWith("/pull");
      if (pull && !s.device_id) fail(403, "DEVICE_REQUIRED");
      const cursor = c.req.query("cursor") ?? "";
      let after: [string, number, number] = ["", 0, -1];
      if (cursor && cursor !== "0") {
        try {
          after = z
            .tuple([z.uuid(), z.number().int().nonnegative(), z.number().int().min(-1).max(0x27f)])
            .parse(JSON.parse(new TextDecoder().decode(unb64(cursor))));
        } catch {
          return fail(400, "INVALID_CURSOR");
        }
      }
      const from = c.req.query("from") ?? "",
        to = c.req.query("to") ?? "9999",
        timezone = c.req.query("timezone") ?? "UTC";
      if (
        (from && !/^\d{4}-\d{2}-\d{2}$/.test(from)) ||
        (to !== "9999" && !/^\d{4}-\d{2}-\d{2}$/.test(to)) ||
        from > to
      )
        fail(400, "INVALID_PERIOD");
      try {
        new Intl.DateTimeFormat("en", { timeZone: timezone });
      } catch {
        return fail(400, "INVALID_TIMEZONE");
      }
      const exclude = c.req.query("exclude_device") ?? "";
      const device = c.req.query("device") ?? "";
      const rows = await all<{
        device_id: string;
        recording_id: string;
        hour_start: number;
        key_code: number;
        value: number;
        started_at: number;
        ended_at: number | null;
        key_schema_version: number;
        data_generation: number;
      }>(
        c.env.DB,
        `SELECT d.id device_id,h.*,r.started_at,r.ended_at,r.key_schema_version,r.data_generation FROM hourly h JOIN recordings r ON r.id=h.recording_id JOIN devices d ON d.id=r.device_id WHERE d.user_id=? AND r.data_generation=? AND (?='' OR d.id=?) AND (?='' OR d.id<>?) AND (h.recording_id,h.hour_start,h.key_code)>(?,?,?) ORDER BY h.recording_id,h.hour_start,h.key_code LIMIT 1001`,
        s.user_id,
        s.data_generation,
        pull ? s.device_id : device,
        pull ? s.device_id : device,
        exclude,
        exclude,
        ...after,
      );
      const page = rows
        .slice(0, 1000)
        .filter(
          (r) =>
            pull ||
            (localDay(r.hour_start, timezone) >= from && localDay(r.hour_start, timezone) <= to),
        )
        .map((r) => ({
          device_id: r.device_id,
          recording_id: r.recording_id,
          hour_start: r.hour_start,
          value: r.value,
          key_code: r.key_code < 0 ? null : r.key_code,
        }));
      const recordings = pull
        ? Array.from(
            new Map(
              rows.slice(0, 1000).map((r) => [
                r.recording_id,
                {
                  id: r.recording_id,
                  started_at: r.started_at,
                  ended_at: r.ended_at,
                  key_schema_version: r.key_schema_version,
                  data_generation: r.data_generation,
                },
              ]),
            ).values(),
          )
        : [];
      const last = rows[999];
      return c.json({
        rows: page,
        recordings,
        generation: s.data_generation,
        next_cursor:
          rows.length > 1000
            ? b64(
                new TextEncoder().encode(
                  JSON.stringify([last!.recording_id, last!.hour_start, last!.key_code]),
                ),
              )
            : null,
      });
    });
  return app;
}
export default createApi();
