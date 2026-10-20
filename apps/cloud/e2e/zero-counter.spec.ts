import { test, expect } from "@playwright/test";
import { createHash, createPrivateKey, sign, randomBytes } from "node:crypto";
import { isoCBOR } from "@simplewebauthn/server/helpers";
test("valid synced Passkey assertions with counter zero are accepted repeatedly", async ({
  page,
  context,
  request,
}) => {
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  await page.route("**/api/v1/auth/register/verify", async (route) => {
    const body = route.request().postDataJSON();
    const attestation = isoCBOR.decodeFirst(
      Buffer.from(body.response.response.attestationObject, "base64url"),
    ) as Map<string, unknown>;
    const auth = attestation.get("authData") as Uint8Array;
    auth.fill(0, 33, 37);
    attestation.set("authData", auth);
    body.response.response.attestationObject = Buffer.from(isoCBOR.encode(attestation)).toString(
      "base64url",
    );
    await route.continue({ postData: JSON.stringify(body) });
  });
  const username = `zero_${randomBytes(6).toString("hex")}`;
  await page.goto("/?mode=manage");
  await page.locator("#username").fill(username);
  const registered = page.waitForResponse((r) => r.url().endsWith("/auth/register/verify"));
  await page.locator("#register").click();
  expect((await registered).status()).toBe(200);
  await expect(page.locator("#account")).toBeVisible();
  const { credentials } = await cdp.send("WebAuthn.getCredentials", { authenticatorId });
  const privateKey = createPrivateKey({
    key: Buffer.from(credentials[0].privateKey, "base64"),
    format: "der",
    type: "pkcs8",
  });
  await page.route("**/api/v1/auth/login/verify", async (route) => {
    const body = route.request().postDataJSON();
    const auth = Buffer.from(body.response.response.authenticatorData, "base64url");
    auth.fill(0, 33, 37);
    const clientHash = createHash("sha256")
      .update(Buffer.from(body.response.response.clientDataJSON, "base64url"))
      .digest();
    body.response.response.authenticatorData = auth.toString("base64url");
    body.response.response.signature = sign(
      privateKey.asymmetricKeyType === "ed25519" ? null : "sha256",
      Buffer.concat([auth, clientHash]),
      privateKey,
    ).toString("base64url");
    await route.continue({ postData: JSON.stringify(body) });
  });
  let admin = "";
  for (let i = 0; i < 2; i++) {
    await page.goto("/?mode=manage");
    await page.locator("#username").fill(username);
    const result = page.waitForResponse((r) => r.url().endsWith("/auth/login/verify"));
    await page.locator("#login").click();
    const response = await result;
    expect(response.status()).toBe(200);
    admin = (await response.json()).access_token;
    await expect(page.locator("#account")).toBeVisible();
  }
  expect(
    (
      await request.delete("/api/v1/account", { headers: { Authorization: `Bearer ${admin}` } })
    ).status(),
  ).toBe(200);
});
