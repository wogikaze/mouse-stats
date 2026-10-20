import { chromium } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { writeFile, mkdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";
// Launch the packaged binary hidden; verify diagnostics, lifecycle and autostart, then exit.
// Never consents to input recording; restores preferences after the autostart check.
const active = execFileSync(
  "powershell.exe",
  [
    "-NoProfile",
    "-Command",
    "@(Get-Process -Name mouse-stats -ErrorAction SilentlyContinue).Count",
  ],
  { encoding: "utf8" },
).trim();
if (active !== "0") throw Error("Close the running Mouse Stats instance before this smoke test.");
const child = spawn(resolve("target/release/mouse-stats.exe"), ["--background"], {
  windowsHide: true,
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
      "--remote-debugging-port=19223 --remote-debugging-address=127.0.0.1",
  },
});
let browser;
try {
  for (let attempt = 0; attempt < 30; attempt++) {
    try {
      browser = await chromium.connectOverCDP("http://127.0.0.1:19223", { timeout: 500 });
      break;
    } catch {
      if (child.exitCode !== null)
        throw Error("Native application exited before WebView initialization");
      await delay(500);
    }
  }
  if (!browser) throw Error("WebView debugger did not become available");
  const pages = browser.contexts().flatMap((c) => c.pages());
  const page =
    pages.find((p) => p.url().includes("tauri.localhost") || p.url().includes("tauri://")) ??
    pages[0];
  if (!page) throw Error("Application WebView missing");
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__?.invoke);
  await page.waitForSelector(".shell");
  await page.waitForFunction(() => document.querySelectorAll(".chart canvas").length === 0);
  const result = await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const diagnostics = await invoke("diagnostics");
    const snapshot = await invoke("snapshot");
    return {
      ...diagnostics,
      native_bridge: true,
      consent: snapshot.settings.consent,
      cloud_default_off: !snapshot.settings.sync_enabled,
    };
  });
  if (!result.database_ok || !result.input_ready || result.window_visible)
    throw Error("Native diagnostics failed");
  result.hidden_charts_deferred = true;
  const autostart = await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const settings = (await invoke("snapshot")).settings;
    try {
      await invoke("save_settings", { settings: { ...settings, autostart: !settings.autostart } });
      if ((await invoke("diagnostics")).autostart_enabled === settings.autostart)
        throw Error("Autostart did not change");
    } finally {
      await invoke("save_settings", { settings });
    }
    return (await invoke("diagnostics")).autostart_enabled === settings.autostart;
  });
  if (!autostart) throw Error("Autostart setting was not restored");
  result.autostart_roundtrip = true;
  const second = spawn(resolve("target/release/mouse-stats.exe"), ["--background"], {
    windowsHide: true,
  });
  await Promise.race([new Promise((r) => second.once("exit", r)), delay(5000)]);
  if (second.exitCode !== 0) {
    second.kill();
    throw Error("Single-instance enforcement failed");
  }
  await page.waitForFunction(
    async () => (await window.__TAURI_INTERNALS__.invoke("diagnostics")).window_visible,
  );
  await page.waitForSelector(".chart canvas");
  result.visible_charts_loaded = true;
  execFileSync(
    "powershell.exe",
    ["-NoProfile", "-Command", `(Get-Process -Id ${child.pid}).CloseMainWindow()`],
    { encoding: "utf8" },
  );
  await page.waitForFunction(
    async () => !(await window.__TAURI_INTERNALS__.invoke("diagnostics")).window_visible,
  );
  await page.waitForFunction(() => document.querySelectorAll(".chart canvas").length === 0);
  result.hidden_charts_unmounted = true;
  result.single_instance = true;
  result.close_to_tray = true;
  execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-File",
      "scripts/measure-native.ps1",
      "-ProcessId",
      String(child.pid),
      "-DurationSeconds",
      "2",
      "-IntervalSeconds",
      "1",
      "-OutputPath",
      "test-results/native-resources-smoke.json",
    ],
    { encoding: "utf8", windowsHide: true },
  );
  const resources = JSON.parse(
    (await readFile("test-results/native-resources-smoke.json", "utf8")).replace(/^\uFEFF/, ""),
  );
  if (
    resources.end_reason !== "duration_completed" ||
    resources.samples.length < 2 ||
    resources.peak_sampled_working_set_bytes <= 0
  )
    throw Error("Native resource measurement failed");
  result.resource_measurement = true;
  if (!result.consent) await page.screenshot({ path: "test-results/native-consent.png" });
  await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("quit_app")).catch(() => {});
  await Promise.race([new Promise((r) => child.once("exit", r)), delay(5000)]);
  if (child.exitCode !== 0) throw Error("Native application did not exit cleanly");
  result.clean_exit = true;
  await mkdir("test-results", { recursive: true });
  await writeFile("test-results/native-smoke.json", JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} finally {
  await browser?.close().catch(() => {});
  if (child.exitCode === null) child.kill();
}
