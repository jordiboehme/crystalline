import { defineConfig } from "@hey-api/openapi-ts";

// Types only: the runtime client in src/api is hand written, so the one plugin
// is @hey-api/typescript and nothing else is emitted.
export default defineConfig({
  input: "../crates/service/openapi/fluid-v1.json",
  output: { path: "src/api/gen" },
  plugins: ["@hey-api/typescript"],
});
