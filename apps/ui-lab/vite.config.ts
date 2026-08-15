import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The app connects its vox WebSocket same-origin (ws://localhost:5173/
// org/<slug>/vox) and vite proxies it to the task-server. Same-origin
// matters: Chrome's Local Network Access checks (M138+) gate
// cross-origin ws:// to loopback behind a permission prompt — silently
// stalling the socket in headless runs — while same-origin loopback is
// exempt. Override the target with TASK_SERVER_HTTP when the server
// isn't on 18080.
const TASK_SERVER = process.env.TASK_SERVER_HTTP ?? "http://127.0.0.1:18080";

const voxProxy = {
  "/org": {
    target: TASK_SERVER,
    ws: true,
    changeOrigin: true,
  },
  // Org discovery — the server's well-known endpoint lists the hosted
  // orgs. Same-origin via the proxy for the same LNA reasons as /org.
  "/.well-known": {
    target: TASK_SERVER,
    changeOrigin: true,
  },
} as const;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // The vendored vox runtime is consumed as raw TS source (workspace
  // links), which Vite does NOT pre-bundle by default — first page
  // load transforms every module on demand, one request at a time.
  // Force them through esbuild prebundling so the dev server serves
  // one cached bundle.
  //
  // NOTE: these MUST be the real package names (@bearcove/*). The
  // first version of this list used bare names ("vox-core"), which
  // vite can't resolve — it logs "Failed to resolve dependency:
  // vox-core, present in client 'optimizeDeps.include'" once at
  // startup and silently skips the prebundle, putting all ~54 vendor
  // modules back on the per-request transform path.
  optimizeDeps: {
    include: [
      "@bearcove/vox-core",
      "@bearcove/vox-ws",
      "@bearcove/vox-wire",
      "@bearcove/vox-postcard",
    ],
  },
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  // Bind IPv4 loopback explicitly. Default binding picked [::1] only,
  // and Firefox-family browsers (Zen) stalled ws upgrades for ~40s
  // dialing the v4 loopback that nothing listened on, while Chromium's
  // happy-eyeballs masked it. v4 serves both: browsers resolve
  // localhost -> 127.0.0.1 fine, and v6-preferring stacks fall back.
  server: { host: "127.0.0.1", proxy: voxProxy },
  preview: { host: "127.0.0.1", proxy: voxProxy },
});
