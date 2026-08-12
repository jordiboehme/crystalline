/**
 * The admin client layer: the GitHub connection, the domain lifecycle, a team
 * domain's sync state and the archive round trip.
 *
 * What is pinned here is what a screen cannot see for itself: which path each
 * verb calls, which method it calls it with, what it puts on the wire and what
 * it hands back. The token is the one with a rule of its own - it goes out in
 * the body of one request and exists nowhere else afterwards.
 */

import { describe, expect, it, vi } from "vitest";

import {
  archiveDownloadUrl,
  createDomain,
  disconnectGithub,
  fetchGithubStatus,
  fetchSyncStatus,
  importArchive,
  previewArchive,
  readGithubStatus,
  startGithubConnect,
  submitGithubToken,
  syncDomain,
  syncStatusKey,
  unregisterDomain,
} from "./admin";
import { api } from "./client";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

/** A connected instance with a device flow still running on it. */
const PENDING_STATUS = {
  enabled: true,
  connected: false,
  user: null,
  token_store: null,
  pending: {
    user_code: "ABCD-1234",
    verification_url: "https://github.example/device",
    expires_in_secs: 900,
  },
  error: null,
};

/** A one-entry preview, as the server shapes it. */
const PREVIEW_REPORT = {
  domain: "eng",
  dry_run: true,
  entries: [
    {
      path: "alpha.md",
      status: "invalid",
      permalink: null,
      reason: "the frontmatter is not YAML",
      findings: [
        {
          rule: "E002",
          severity: "error",
          message: "status is required",
          line: 3,
        },
      ],
    },
  ],
  new: 0,
  collides: 0,
  written: 0,
  skipped: 0,
  invalid: 1,
  ignored: 0,
};

describe("the admin client layer", () => {
  it("reads a GitHub status out of its wire spelling", async () => {
    apiMock.mockResolvedValueOnce(PENDING_STATUS);
    const status = await fetchGithubStatus();

    expect(apiMock).toHaveBeenLastCalledWith("/settings/github");
    expect(status).toEqual({
      enabled: true,
      connected: false,
      user: null,
      tokenStore: null,
      pending: {
        userCode: "ABCD-1234",
        verificationUrl: "https://github.example/device",
        expiresInSecs: 900,
      },
      error: null,
    });
  });

  it("reads a connected instance, and a flow that is not running", () => {
    expect(
      readGithubStatus({
        enabled: true,
        connected: true,
        user: "octo",
        token_store: "keyring",
        pending: null,
        error: "authorization denied",
      }),
    ).toEqual({
      enabled: true,
      connected: true,
      user: "octo",
      tokenStore: "keyring",
      pending: null,
      error: "authorization denied",
    });
  });

  it("starts a device flow on its own route", async () => {
    apiMock.mockResolvedValueOnce(PENDING_STATUS);
    const status = await startGithubConnect();

    expect(apiMock).toHaveBeenLastCalledWith(
      "/settings/github/connect",
      expect.objectContaining({ method: "POST" }),
    );
    expect(status.pending?.userCode).toBe("ABCD-1234");
  });

  it("posts a token and keeps no copy of it", async () => {
    apiMock.mockResolvedValueOnce({
      enabled: true,
      connected: true,
      user: "octo",
      token_store: "keyring",
      pending: null,
      error: null,
    });
    const status = await submitGithubToken("ghp_secret");

    expect(apiMock).toHaveBeenLastCalledWith(
      "/settings/github/token",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ token: "ghp_secret" }),
      }),
    );
    // The token leaves in that one body and nowhere else: nothing this module
    // hands back and nothing it holds carries it afterwards.
    expect(JSON.stringify(status)).not.toContain("ghp_secret");
    const module = await import("./admin");
    const held = Object.values(module).filter(
      (value) => typeof value !== "function",
    );
    expect(JSON.stringify(held)).not.toContain("ghp_secret");
  });

  it("forgets the credential with a DELETE", async () => {
    apiMock.mockResolvedValueOnce({ enabled: true, connected: false });
    const status = await disconnectGithub();

    expect(apiMock).toHaveBeenLastCalledWith(
      "/settings/github",
      expect.objectContaining({ method: "DELETE" }),
    );
    expect(status).toEqual({
      enabled: true,
      connected: false,
      user: null,
      tokenStore: null,
      pending: null,
      error: null,
    });
  });

  it("registers a domain and reads where it landed", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "notes",
      root: "/srv/domains/notes",
      kind: "file",
    });
    const created = await createDomain({ mode: "local", name: "notes" });

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ mode: "local", name: "notes" }),
      }),
    );
    expect(created).toEqual({ domain: "notes", root: "/srv/domains/notes" });
  });

  it("unregisters a domain with a DELETE, and reads the receipt", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "team eng",
      files_kept: true,
      rooms_closed: 2,
    });
    const receipt = await unregisterDomain("team eng");

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/team%20eng",
      expect.objectContaining({ method: "DELETE" }),
    );
    expect(receipt).toEqual({ filesKept: true, roomsClosed: 2 });
  });

  it("counts a sync report's lists as well as its numbers", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      branch: "main",
      last_checked: "2026-08-10T08:00:00Z",
      local_changes: 2,
      // The engine's status report carries the proposals themselves here; the
      // card wants how many there are, so a list counts as its own length.
      open_proposals: [{ number: 7 }, { number: 9 }],
      behind: false,
    });
    const status = await fetchSyncStatus("eng");

    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/sync");
    expect(status).toEqual({
      repo: "acme/kb",
      branch: "main",
      lastChecked: "2026-08-10T08:00:00Z",
      localChanges: 2,
      openProposals: 2,
      behind: false,
      probeError: null,
    });
    expect(syncStatusKey("eng")).toEqual(["domains", "eng", "sync"]);
  });

  it("carries a failed probe's own words through", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      branch: "main",
      last_checked: "2026-08-09T08:00:00Z",
      local_changes: 2,
      open_proposals: [],
      // The report came back without the probe, so `behind` is unknown and
      // everything beside it is local state alone.
      behind: null,
      probe_error: "offline: could not reach api.github.com",
    });
    const status = await fetchSyncStatus("eng");

    expect(status.probeError).toBe("offline: could not reach api.github.com");
    expect(status.behind).toBeNull();
  });

  it("pulls a team domain on the same path with a POST", async () => {
    apiMock.mockResolvedValueOnce({ domain: "eng", applied: 1 });
    await syncDomain("eng");

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/sync",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("percent-encodes the archive download address", () => {
    expect(archiveDownloadUrl("a b")).toBe("/api/v1/domains/a%20b/archive");
  });

  it("previews an upload as zip bytes, not as JSON", async () => {
    const data = new ArrayBuffer(4);
    apiMock.mockResolvedValueOnce(PREVIEW_REPORT);
    const report = await previewArchive("eng", data);

    // The explicit content type is the whole point: the client only defaults
    // an unset one to JSON, and these bytes are not JSON.
    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/archive/preview", {
      method: "POST",
      body: data,
      headers: { "Content-Type": "application/zip" },
    });
    expect(report).toEqual({
      entries: [
        {
          path: "alpha.md",
          status: "invalid",
          permalink: null,
          reason: "the frontmatter is not YAML",
          findings: [
            {
              rule: "E002",
              severity: "error",
              message: "status is required",
              line: 3,
            },
          ],
        },
      ],
      written: 0,
      skipped: 0,
      invalid: 1,
      ignored: 0,
    });
  });

  it("names the policy on the import only when it is not the default", async () => {
    const data = new ArrayBuffer(4);
    apiMock.mockResolvedValue({ ...PREVIEW_REPORT, dry_run: false });

    await importArchive("eng", data, "skip");
    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/archive/import", {
      method: "POST",
      body: data,
      headers: { "Content-Type": "application/zip" },
    });

    await importArchive("eng", data, "overwrite");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/archive/import?policy=overwrite",
      {
        method: "POST",
        body: data,
        headers: { "Content-Type": "application/zip" },
      },
    );
  });
});
