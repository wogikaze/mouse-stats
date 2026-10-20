import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("../node_modules/void/dist/cli/cli.mjs", import.meta.url));
const result = spawnSync(process.execPath, [cli, "dev", "--host", "127.0.0.1", "--port", "5173"], {
  cwd: fileURLToPath(new URL("../apps/cloud/", import.meta.url)),
  stdio: "inherit",
  env: { ...process.env, MOUSE_STATS_ORIGIN: "http://localhost:5173" },
});
process.exit(result.status ?? 1);
