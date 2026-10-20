import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "e2e",
  timeout: 60000,
  workers: 1,
  use: { baseURL: "http://localhost:5173", channel: "msedge", headless: true },
  reporter: "list",
  outputDir: "../../test-results/cloud",
  webServer: {
    command: "npm run dev",
    url: "http://localhost:5173/api/health",
    reuseExistingServer: !process.env.CI,
    timeout: 60000,
  },
});
