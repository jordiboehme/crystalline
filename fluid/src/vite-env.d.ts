/// <reference types="vite/client" />

interface ImportMetaEnv {
  /**
   * The build's own version, frozen in by `define` in vite.config.ts from
   * package.json. Compared against the server version from `GET /auth/me` to
   * warn about a browser holding a stale build.
   */
  readonly VITE_APP_VERSION: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
