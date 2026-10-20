import { defineConfig } from "void/config";
const origin = new URL(process.env.MOUSE_STATS_ORIGIN ?? "http://localhost:5173");
export default defineConfig({
  target: "cloudflare",
  inference: { appType: "spa", bindings: { db: true } },
  worker: {
    compatibility_date: "2026-10-19",
    compatibility_flags: ["nodejs_compat"],
    vars: { ORIGIN: origin.origin, RP_ID: origin.hostname },
  },
  cloudflare: { name: "mouse-stats", account_id: process.env.CLOUDFLARE_ACCOUNT_ID || undefined },
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
