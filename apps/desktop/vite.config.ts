import { defineConfig } from "vite-plus";
import vue from "@vitejs/plugin-vue";
import tailwind from "@tailwindcss/vite";
export default defineConfig({
  plugins: [vue(), tailwind()],
  clearScreen: false,
  server: { strictPort: true, port: 1420, host: "127.0.0.1" },
  envPrefix: ["VITE_", "TAURI_ENV_"],
});
