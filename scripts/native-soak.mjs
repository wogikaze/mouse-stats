import { chromium } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index < 0 ? fallback : process.argv[index + 1];
}

const durationSeconds = Number(option("--duration-seconds", "1800"));
const outputPath = resolve(option("--output", "test-results/native-resources-soak.json"));
if (!Number.isInteger(durationSeconds) || durationSeconds < 1 || durationSeconds > 86400)
  throw Error("--duration-seconds must be an integer between 1 and 86400.");

const active = execFileSync(
  "powershell.exe",
  [
    "-NoProfile",
    "-Command",
    "@(Get-Process -Name mouse-stats -ErrorAction SilentlyContinue).Count",
  ],
  { encoding: "utf8" },
).trim();
if (active !== "0") throw Error("Close the running Mouse Stats instance before this soak test.");

const child = spawn(resolve("target/release/mouse-stats.exe"), ["--background"], {
  windowsHide: true,
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
      "--remote-debugging-port=19223 --remote-debugging-address=127.0.0.1",
  },
});
let browser;
let page;
try {
  for (let attempt = 0; attempt < 60; attempt++) {
    try {
      browser = await chromium.connectOverCDP("http://127.0.0.1:19223", { timeout: 500 });
      break;
    } catch {
      if (child.exitCode !== null)
        throw Error("Native application exited before WebView initialization.");
      await delay(500);
    }
  }
  if (!browser) throw Error("WebView debugger did not become available.");
  page = browser.contexts().flatMap((context) => context.pages())[0];
  if (!page) throw Error("Application WebView missing.");
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__?.invoke);
  const initial = await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    return {
      diagnostics: await invoke("diagnostics"),
      settings: (await invoke("snapshot")).settings,
    };
  });
  if (
    !initial.diagnostics.database_ok ||
    !initial.diagnostics.input_ready ||
    initial.diagnostics.window_visible
  )
    throw Error("Native app did not start hidden with its database and input listener ready.");
  if (!initial.settings.consent || initial.settings.paused || initial.settings.sync_enabled)
    throw Error("Soak test requires saved consent, active measurement, and cloud sync turned off.");

  execFileSync(
    "powershell.exe",
    [
      "-NoProfile",
      "-File",
      "scripts/measure-native.ps1",
      "-ProcessId",
      String(child.pid),
      "-DurationSeconds",
      String(durationSeconds),
      "-IntervalSeconds",
      "5",
      "-OutputPath",
      outputPath,
    ],
    { encoding: "utf8", windowsHide: true, timeout: (durationSeconds + 60) * 1000 },
  );
  const resources = await readFile(outputPath, "utf8");
  const report = JSON.parse(resources.replace(/^\uFEFF/, ""));
  if (report.end_reason !== "duration_completed" || report.elapsed_seconds < durationSeconds - 2)
    throw Error("The native process did not complete the requested soak duration.");

  report.limitations = report.limitations.filter(
    (limitation) => !limitation.includes("Does not verify physical input"),
  );
  report.limitations.push(
    "The launcher verifies saved consent, active measurement, sync disabled, hidden window, and input listener readiness. Physical input, polling rate, and event loss still require manual verification.",
  );

  await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("quit_app"));
  await Promise.race([new Promise((resolveExit) => child.once("exit", resolveExit)), delay(10000)]);
  if (child.exitCode !== 0)
    throw Error("Native application did not exit cleanly after measurement.");
  report.consent_verified = true;
  report.cloud_sync_verified_off = true;
  report.window_hidden = true;
  report.input_listener_ready = true;
  report.clean_exit = true;
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ ...report, samples: `${report.samples.length} samples` }));
} finally {
  await page?.evaluate(() => window.__TAURI_INTERNALS__.invoke("quit_app")).catch(() => {});
  await browser?.close().catch(() => {});
  if (child.exitCode === null) child.kill();
}
