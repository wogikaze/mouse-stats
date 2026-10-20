import { test, expect } from "@playwright/test";
test("dashboard, filters, physical keyboard, settings and deletion confirmation", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.addInitScript(() => {
    const hour = Math.floor(Date.now() / 3600000) * 3600;
    const splitHour = Math.floor(Date.now() / 86400000) * 86400 - 6 * 3600;
    const state = {
      device_id: "local",
      recording_id: "r",
      settings: {
        consent: true,
        paused: false,
        autostart: false,
        sync_enabled: false,
        upload_history: false,
        timezone: "Asia/Kolkata",
        layout: "US",
        cloud_url: "",
      },
      rows: [
        { device_id: "local", recording_id: "r", hour_start: splitHour, key_code: 30, value: 12 },
        { device_id: "remote", recording_id: "s", hour_start: hour, key_code: 30, value: 7 },
        { device_id: "local", recording_id: "r", hour_start: hour, key_code: null, value: 5000 },
      ],
      devices: [{ id: "remote", name: "Other PC", last_seen: hour, revoked_at: null }],
      last_saved: hour,
      last_sync: hour,
      sync_status: "ローカルのみ",
      error: null,
      generation_conflict: false,
      signed_in: false,
    };
    const testWindow = window as Window & { __deleteLocalFails: boolean };
    testWindow.__deleteLocalFails = true;
    Object.assign(window, {
      __TAURI_INTERNALS__: {
        invoke: async (cmd: string, args: Record<string, unknown>) => {
          if (cmd === "snapshot") return structuredClone(state);
          if (cmd === "save_settings") {
            state.settings = JSON.parse(JSON.stringify(args.settings));
            return;
          }
          if (cmd === "delete_local") {
            if (testWindow.__deleteLocalFails) throw Error("synthetic database failure");
            state.rows = [];
            return;
          }
          throw Error(`Unexpected command: ${cmd}`);
        },
      },
    });
  });
  await page.goto("/");
  await expect(page.locator(".big").first()).toContainText("19");
  await expect(page.getByRole("status")).toContainText(
    "UTCの1時間枠がローカル日付をまたいでいます",
  );
  await expect(page.locator(".calendar>span")).toHaveCount(364);
  await expect(page.locator(".chart canvas").first()).toBeVisible();
  await page.getByLabel("デバイス", { exact: true }).selectOption("local");
  await expect(page.locator(".big").first()).toContainText("12");
  await page.getByRole("button", { name: "キーボード分析", exact: false }).click();
  await expect(page.locator(".key").filter({ hasText: /^A12$/ })).toBeVisible();
  await page.getByRole("button", { name: "設定・アカウント", exact: false }).click();
  await expect(page.getByRole("heading", { name: "計測と表示" })).toBeVisible();
  await page.getByLabel("キーボード配列", { exact: true }).selectOption("JIS");
  await page.getByRole("button", { name: "設定を保存", exact: true }).click();
  await page.getByRole("button", { name: "キーボード分析", exact: false }).click();
  await expect(page.locator(".key").filter({ hasText: "かな" })).toBeVisible();
  await page.screenshot({ path: "test-results/desktop/keyboard.png", fullPage: true });
  await page.getByRole("button", { name: "ダッシュボード", exact: false }).click();
  await page.screenshot({ path: "test-results/desktop/dashboard.png", fullPage: true });
  await page.getByRole("button", { name: "設定・アカウント", exact: false }).click();
  await page.getByRole("button", { name: "このPCの統計を削除", exact: true }).click();
  await expect(page.getByRole("button", { name: "削除する", exact: true })).toBeDisabled();
  await page.getByLabel("操作確認").fill("削除");
  await page.getByRole("button", { name: "削除する", exact: true }).click();
  await expect(page.getByRole("heading", { name: "統計を削除しますか？" })).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("synthetic database failure");
  await page.evaluate(() => {
    (window as Window & { __deleteLocalFails: boolean }).__deleteLocalFails = false;
  });
  await page.getByRole("button", { name: "削除する", exact: true }).click();
  await expect(page.getByRole("heading", { name: "統計を削除しますか？" })).toHaveCount(0);
  await page.getByRole("button", { name: "ダッシュボード", exact: false }).click();
  await expect(page.locator(".big").first()).toContainText("0");
  expect(errors).toEqual([]);
});
