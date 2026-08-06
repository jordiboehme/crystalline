/**
 * The browser smoke's configuration.
 *
 * What this suite is for is what jsdom cannot answer: a real bundle served by
 * a real static server, talking to a real Crystalline daemon, in a real
 * browser. So it runs against `vite preview` rather than the dev server, and
 * against the build in `dist/` rather than the source - the artifact a
 * deployment actually ships is the one under test.
 *
 * The API is not stubbed. `vite preview` inherits `server.proxy` from
 * `vite.config.ts`, so `/api` is forwarded to the daemon on 127.0.0.1:7411
 * exactly as nginx forwards it in the image, and `e2e/run-smoke.sh` is what
 * puts a daemon with a fixture domain and an account behind that port. Run the
 * suite through that script; `pnpm exec playwright test` on its own has
 * nothing to talk to.
 */

import { defineConfig, devices } from "@playwright/test";

/** Where the preview server answers. The daemon's own port is fixed at 7411. */
const PORT = Number(process.env.FLUID_PREVIEW_PORT ?? "4173");
const BASE_URL = `http://127.0.0.1:${String(PORT)}`;

export default defineConfig({
  testDir: "./e2e",
  // One browser, one worker: the suite shares one daemon holding one fixture
  // domain, and nothing in it is a throughput test.
  workers: 1,
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  // A flake here is a bug in the app or in the fixture, and a retry would hide
  // whichever it is.
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    // The built bundle, served the way it is served in production: no dev
    // server, no source transforms, no HMR client.
    //
    // The host is named rather than left to `localhost`, which is two
    // addresses: on a machine that resolves it to ::1 first, `vite preview`
    // listens there alone and every probe of 127.0.0.1 is refused. Pinning it
    // to the address the tests use makes the two the same one.
    command: `pnpm exec vite preview --host 127.0.0.1 --port ${String(PORT)} --strictPort`,
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
