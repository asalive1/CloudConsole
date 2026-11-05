import pkg from "./package.json";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  server: {
    host: "0.0.0.0",
    port: 5173,
    strictPort: true,
  },
});
