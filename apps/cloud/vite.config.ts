import { defineConfig } from "vite-plus";
import { voidPlugin } from "void";
export default defineConfig({
  plugins: [voidPlugin()],
  server: { port: 5173, strictPort: true, host: "127.0.0.1" },
});
