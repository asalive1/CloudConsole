import pkg from "./package.json";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

// janus-gateway assigns module.exports inside an ES module. Rollup then treats the file as
// CommonJS and its default export becomes undefined in production builds.
const janusEsmFix: Plugin = {
  name: "janus-esm-fix",
  enforce: "pre",
  transform(code, id) {
    if (!id.includes("janus-gateway") || !id.endsWith("janus.es.js")) return null;
    const patched = code.replace("module.exports = factory();", "return factory();");
    if (patched === code) throw new Error("janus-esm-fix: expected pattern not found");
    return { code: patched, map: null };
  },
};

export default defineConfig({
  plugins: [janusEsmFix, react()],
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  server: {
    host: "0.0.0.0",
    port: 5173,
    strictPort: true,
  },
});
