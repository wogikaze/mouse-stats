import { defineConfig } from "void/config";
// Cloudflare's Git-connected builds evaluate this config without the local
// deploy wrapper. Default to the production origin so those builds cannot
// publish localhost as the WebAuthn origin.
const origin = new URL(
  process.env.MOUSE_STATS_ORIGIN ?? "https://mouse-stats.kouse2319.workers.dev",
);
export default defineConfig({
  target: "cloudflare",
  inference: { appType: "void", bindings: { db: true } },
  worker: {
    compatibility_date: "2026-10-19",
    compatibility_flags: ["nodejs_compat"],
    vars: { ORIGIN: origin.origin, RP_ID: origin.hostname },
  },
  cloudflare: {
    name: "mouse-stats",
    account_id: process.env.CLOUDFLARE_ACCOUNT_ID ?? "05e7a2aa0fa3b9aff9b9217e9b6ff743",
    routes: [],
    workers_dev: true,
    preview_urls: true,
  },
  routing: {
    headers: {
      "/*": [
        "X-Frame-Options: DENY",
        "X-Content-Type-Options: nosniff",
        "Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
      ],
    },
  },
});
