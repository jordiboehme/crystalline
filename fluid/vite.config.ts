import { readFileSync } from "node:fs";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

/**
 * The app's own version, read from package.json at config time and frozen into
 * the bundle as `import.meta.env.VITE_APP_VERSION`.
 *
 * It is kept equal to the workspace Cargo version by hand, so the UI can
 * compare itself against the `version` field `GET /auth/me` returns and warn
 * when a browser is holding an older build than the server it talks to.
 */
const { version } = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

/**
 * The local `crystalline serve --http` a `pnpm dev` session talks to. It is the
 * daemon's own default bind (`DEFAULT_HTTP_ADDR` in
 * crates/service/src/daemon.rs).
 */
const DEV_API_TARGET = "http://127.0.0.1:7411";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  define: {
    "import.meta.env.VITE_APP_VERSION": JSON.stringify(version),
  },
  server: {
    // Same-origin in development too: the browser only ever talks to the Vite
    // server, which forwards /api to the real daemon. The API has no CORS
    // layer by design and must never get one, so a cross-origin dev setup
    // would not work and would misrepresent production.
    //
    // `changeOrigin` stays off on purpose: the daemon reads the Host header to
    // decide whether the session cookie needs the Secure flag, and the
    // browser's own loopback Host is the answer that keeps a plain http dev
    // session working.
    proxy: {
      "/api": {
        target: DEV_API_TARGET,
      },
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
