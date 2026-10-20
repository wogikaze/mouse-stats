import { defineConfig } from "vite-plus";
export default defineConfig({
  test: { include: ["packages/**/*.test.ts", "apps/cloud/tests/**/*.test.ts"] },
  lint: { ignorePatterns: ["**/.void/**", "**/dist/**", "**/target/**", "**/gen/**"] },
  fmt: {
    ignorePatterns: [
      "**/.void/**",
      "**/dist/**",
      "**/target/**",
      "**/gen/**",
      "**/.void-wrangler.jsonc",
      "PLAN.md",
    ],
  },
});
