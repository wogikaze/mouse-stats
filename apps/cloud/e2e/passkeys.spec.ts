import { test, expect } from "@playwright/test";
import { randomBytes } from "node:crypto";
test("add, rename and revoke Passkeys through browser management", async ({
  page,
  context,
  request,
}) => {
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const options = {
    protocol: "ctap2",
    transport: "internal",
    hasResidentKey: true,
    hasUserVerification: true,
    isUserVerified: true,
    automaticPresenceSimulation: true,
  };
  const first = await cdp.send("WebAuthn.addVirtualAuthenticator", { options });
  await page.route("**/api/v1/stats?*", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        rows: [
          {
            device_id: randomBytes(16).toString("hex"),
            recording_id: randomBytes(16).toString("hex"),
            hour_start: Math.floor(Date.now() / 3600000) * 3600,
            key_code: 30,
            value: 123,
          },
          {
            device_id: randomBytes(16).toString("hex"),
            recording_id: randomBytes(16).toString("hex"),
            hour_start: Math.floor(Date.now() / 3600000) * 3600,
            key_code: null,
            value: 4567,
          },
        ],
        next_cursor: null,
      }),
    }),
  );
  await page.goto("/?mode=manage");
  await page.locator("#username").fill(`keys_${randomBytes(6).toString("hex")}`);
  const verification = page.waitForResponse((r) => r.url().endsWith("/auth/register/verify"));
  await page.locator("#register").click();
  const verified = await verification;
  expect(verified.status()).toBe(200);
  const admin = (await verified.json()).access_token;
  await expect(page.locator("#account")).toBeVisible();
  await expect(page.locator("#stats-keys")).toHaveText("123");
  await expect(page.locator("#stats-mouse")).toHaveText("4,567");
  await expect(page.locator("#stats-empty")).toBeHidden();
  await expect(page.locator("#stats-ranking")).toContainText("A");
  await cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: first.authenticatorId });
  await cdp.send("WebAuthn.addVirtualAuthenticator", { options });
  await page.locator("#key-name").fill("予備のPasskey");
  await page.locator("#add-key").click();
  await expect(page.locator("#passkeys .item")).toHaveCount(2);
  const items = page.locator("#passkeys .item");
  await items.first().locator("input").fill("メインの鍵");
  await items.first().getByRole("button", { name: "名前を保存" }).click();
  await expect(page.locator("#status")).toContainText("名前を変更しました");
  const list = await (
    await request.get("/api/v1/passkeys", { headers: { Authorization: `Bearer ${admin}` } })
  ).json();
  const key = list.passkeys.find((k: { name: string }) => k.name === "メインの鍵");
  expect(key).toBeTruthy();
  await items.first().getByRole("button", { name: "失効", exact: true }).click();
  await expect(page.locator("#passkeys button").filter({ hasText: "失効済み" })).toHaveCount(1);
  await items.nth(1).getByRole("button", { name: "失効", exact: true }).click();
  await expect(page.locator("#passkeys button").filter({ hasText: "失効済み" })).toHaveCount(1);
  const response = await request.delete("/api/v1/account", {
    headers: { Authorization: `Bearer ${admin}` },
  });
  expect(response.status()).toBe(200);
});
