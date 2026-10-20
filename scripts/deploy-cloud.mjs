import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
const raw = process.env.MOUSE_STATS_ORIGIN;
if (!raw) throw Error("Set MOUSE_STATS_ORIGIN to the fixed HTTPS origin before deployment.");
const origin = new URL(raw);
if (
  origin.protocol !== "https:" ||
  origin.hostname === "localhost" ||
  origin.username ||
  origin.password ||
  origin.pathname !== "/" ||
  origin.search ||
  origin.hash
)
  throw Error("MOUSE_STATS_ORIGIN must be an HTTPS origin.");
const cli = fileURLToPath(new URL("../node_modules/void/dist/cli/cli.mjs", import.meta.url));
const result = spawnSync(process.execPath, [cli, "deploy", "--platform", "cloudflare"], {
  cwd: fileURLToPath(new URL("../apps/cloud/", import.meta.url)),
  stdio: "inherit",
  env: process.env,
});
process.exit(result.status ?? 1);
