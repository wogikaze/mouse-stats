import { test, expect } from "@playwright/test";
import { createHash, randomBytes, randomUUID } from "node:crypto";
test("Passkey + PKCE, two PCs, retry, refresh, recovery, reset and revoke on real D1 runtime", async ({
  page,
  context,
  request,
}) => {
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  const username = `e2e_${randomBytes(6).toString("hex")}`;
  await page.goto("/?mode=manage");
  await page.locator("#username").fill(username);
  const registration = page.waitForResponse((r) => r.url().endsWith("/auth/register/verify"));
  await page.locator("#register").click();
  const registered = await registration;
  expect(registered.status()).toBe(200);
  const admin = (await registered.json()).access_token;
  await expect(page.locator("#account")).toBeVisible();
  const call = async (
    path: string,
    body?: unknown,
    token = admin,
    method = body ? "POST" : "GET",
  ) =>
    request.fetch(`/api/v1${path}`, {
      method,
      headers: { Authorization: `Bearer ${token}` },
      ...(body ? { data: body } : {}),
    });
  const codes = await (await call("/recovery", {})).json();
  const recover = await call("/auth/recover", { username, code: codes.codes[0] });
  expect(recover.status()).toBe(200);
  expect((await call("/auth/recover", { username, code: codes.codes[0] })).status()).toBe(401);
  async function connect(device_id: string) {
    const verifier = randomBytes(32).toString("base64url"),
      pkce = createHash("sha256").update(verifier).digest("base64url"),
      state = randomBytes(32).toString("base64url");
    await page.goto(`/?mode=device&state=${state}&pkce=${pkce}`);
    await page.locator("#username").fill(username);
    const response = page.waitForResponse((r) => r.url().endsWith("/auth/login/verify"));
    await page.locator("#login").click();
    const result = await response;
    expect(result.status()).toBe(200);
    const { code } = await result.json();
    const identity = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
      "sign",
      "verify",
    ]);
    const recovery_key = Buffer.from(
      await crypto.subtle.exportKey("raw", identity.publicKey),
    ).toString("base64url");
    const body = {
      code,
      verifier,
      state,
      device_id,
      name: `Test ${device_id.slice(0, 4)}`,
      recovery_key,
    };
    expect(
      (
        await call("/auth/device/exchange", {
          ...body,
          verifier: randomBytes(32).toString("base64url"),
        })
      ).status(),
    ).toBe(401);
    const exchanged = await call("/auth/device/exchange", body);
    expect(exchanged.status()).toBe(200);
    expect((await call("/auth/device/exchange", body)).status()).toBe(401);
    return { ...(await exchanged.json()), identity };
  }
  const deviceA = randomUUID(),
    deviceB = randomUUID();
  const a = await connect(deviceA),
    b = await connect(deviceB);
  const recordingA = randomUUID(),
    recordingB = randomUUID(),
    hour = Math.floor(Date.now() / 3600000) * 3600;
  const batch = (id: string, value: number, generation = 0) => ({
    generation,
    recordings: [
      { id, started_at: hour, ended_at: null, key_schema_version: 1, data_generation: generation },
    ],
    rows: [
      { recording_id: id, hour_start: hour, key_code: 30, value, revision: 1 },
      { recording_id: id, hour_start: hour, key_code: null, value: value * 1000, revision: 1 },
    ],
  });
  for (const value of [10, 10, 8, 12])
    expect((await call("/sync/push", batch(recordingA, value), a.access_token)).status()).toBe(200);
  expect((await call("/sync/push", batch(recordingB, 7), b.access_token)).status()).toBe(200);
  const combined = await (await call("/stats", undefined, a.access_token)).json();
  expect(
    combined.rows
      .filter((r: { key_code: number | null }) => r.key_code !== null)
      .reduce((n: number, r: { value: number }) => n + r.value, 0),
  ).toBe(19);
  const remote = await (
    await call(`/stats?exclude_device=${deviceA}`, undefined, a.access_token)
  ).json();
  expect(remote.rows).toHaveLength(2);
  expect(remote.rows.every((r: { device_id: string }) => r.device_id === deviceB)).toBe(true);
  const own = await (await call("/sync/pull", undefined, a.access_token)).json();
  expect(own.rows.every((r: { device_id: string }) => r.device_id === deviceA)).toBe(true);
  const nonce = randomBytes(32).toString("base64url");
  const rotated = await call("/auth/refresh", { refresh_token: a.refresh_token, nonce });
  expect(rotated.status()).toBe(200);
  expect(
    (
      await call("/auth/refresh", {
        refresh_token: a.refresh_token,
        nonce: randomBytes(32).toString("base64url"),
      })
    ).status(),
  ).toBe(401);
  const fresh = await rotated.json();
  const retry = await (
    await call("/auth/refresh", { refresh_token: a.refresh_token, nonce })
  ).json();
  expect(retry.refresh_token).toBe(fresh.refresh_token);
  const recoveryOptions = await (
    await call("/device-recovery/options", {}, fresh.access_token)
  ).json();
  const signature = Buffer.from(
    await crypto.subtle.sign(
      { name: "ECDSA", hash: "SHA-256" },
      a.identity.privateKey,
      new TextEncoder().encode(recoveryOptions.challenge),
    ),
  ).toString("base64url");
  const verified = await call(
    "/device-recovery/verify",
    { challenge_id: recoveryOptions.challenge_id, signature },
    fresh.access_token,
  );
  expect(verified.status()).toBe(200);
  const management = await call("/auth/device-recovery/exchange", {
    code: (await verified.json()).code,
  });
  expect(management.status()).toBe(200);
  expect((await call("/data/reset", {})).status()).toBe(200);
  expect((await call("/sync/push", batch(recordingB, 99), b.access_token)).status()).toBe(409);
  expect((await (await call("/stats", undefined, fresh.access_token)).json()).rows).toHaveLength(0);
  expect((await call(`/devices/${deviceB}/revoke`, {})).status()).toBe(200);
  expect((await call("/sync/push", batch(randomUUID(), 1, 1), b.access_token)).status()).toBe(401);
  expect((await call("/account", undefined, admin, "DELETE")).status()).toBe(200);
  expect((await call("/stats", undefined, fresh.access_token)).status()).toBe(401);
});
