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
  fetchConflict,
  fetchGithubStatus,
  fetchShareChanges,
  fetchSyncStatus,
  importArchive,
  previewArchive,
  readGithubStatus,
  resolveConflict,
  shareDomain,
  startGithubConnect,
  submitGithubToken,
  syncDomain,
  syncStatusKey,
  unregisterDomain,
  withdrawProposal,
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
      // Nothing declined and nothing conflicting is nothing to count: a report
      // that leaves the keys out says zero rather than "unknown".
      declinedProposals: 0,
      conflicts: 0,
      behind: false,
      probeError: null,
      // Neither key was sent, and neither is invented: a report with no mode
      // and no connection block says nothing about either rather than
      // guessing "github" and "connected".
      mode: null,
      connected: null,
      // The same two records the count above was taken from, as themselves.
      // Everything the fixture left out is filled in rather than left
      // undefined: a row draws a title and a status whatever arrived. The
      // default status is `open` for both lists, not the list's own name - a
      // record that arrives with no status at all is a report this side cannot
      // read, and guessing `declined` from the key it came under would put a
      // word on a chip that nothing said.
      proposals: [
        {
          number: 7,
          url: "",
          title: "",
          status: "open",
          reviewState: null,
          amendedUpstream: false,
          feedback: [],
          updatedAt: null,
        },
        {
          number: 9,
          url: "",
          title: "",
          status: "open",
          reviewState: null,
          amendedUpstream: false,
          feedback: [],
          updatedAt: null,
        },
      ],
      conflictList: [],
    });
    expect(syncStatusKey("eng")).toEqual(["domains", "eng", "sync"]);
  });

  it("reads the mode and the connection the sync report carries", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      mode: "github",
      repo: "acme/kb",
      branch: "main",
      local_changes: 0,
      open_proposals: [],
      behind: false,
      connection: { connected: true, user: "octo", token_store: "keychain" },
    });
    const status = await fetchSyncStatus("eng");

    expect(status.mode).toBe("github");
    expect(status.connected).toBe(true);
  });

  it("reads an instance with no credential on file as not connected", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      mode: "github",
      repo: "acme/kb",
      local_changes: 0,
      open_proposals: [],
      // The route reports a missing connection rather than refusing over it,
      // so `false` is an answer the card has to be able to say out loud.
      connection: { connected: false },
      probe_error: "no GitHub connection on this instance",
    });
    const status = await fetchSyncStatus("eng");

    expect(status.connected).toBe(false);
  });

  it("reads a connection block of nonsense as no answer rather than as false", async () => {
    // Absent, or present and unreadable, both mean "this report does not say".
    // Only a literal boolean is an answer, because the card acts on `false`.
    apiMock.mockResolvedValueOnce({
      repo: "acme/kb",
      connection: "nonsense",
    });
    expect((await fetchSyncStatus("eng")).connected).toBeNull();

    apiMock.mockResolvedValueOnce({
      repo: "acme/kb",
      connection: { connected: "yes" },
    });
    expect((await fetchSyncStatus("eng")).connected).toBeNull();
  });

  it("counts the declined proposals and the conflicts as well", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      branch: "main",
      last_checked: "2026-08-10T08:00:00Z",
      local_changes: 0,
      open_proposals: [],
      // The two exceptional lists, in the spelling `status_report_json` sends:
      // the records themselves, which the card wants as counts. A conflict is
      // the wire record and not the path string it is easy to mistake it for -
      // counting by length works either way, so the fixture carries the real
      // shape to keep the reader honest about what it is reading past.
      declined_proposals: [{ number: 3 }, { number: 4 }],
      conflicts: [
        {
          id: "9f3c1ab0",
          path: "notes/a.md",
          kind: "EditEdit",
          base_commit: "1111111111111111111111111111111111111111",
          upstream_commit: "2222222222222222222222222222222222222222",
          detected_at: "2026-08-10T07:59:00Z",
        },
      ],
      behind: false,
    });
    const status = await fetchSyncStatus("eng");

    expect(status.declinedProposals).toBe(2);
    expect(status.conflicts).toBe(1);
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

  it("reads the proposals and the conflicts as themselves, both lists as one", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      open_proposals: [
        {
          number: 4,
          url: "https://github.example/acme/kb/pull/4",
          title: "Refine 2 engrams",
          // PascalCase on the wire, because the engine spells three of the
          // four states that way and only `withdrawn` lowercase.
          status: "Open",
          review_state: "changes_requested",
          amended_upstream: true,
          feedback: [
            {
              author: "ana",
              body: "needs a source",
              path: "notes/a.md",
              line: 12,
              submitted_at: "2026-08-21T10:00:00Z",
              kind: "review_comment",
            },
            // No body is nothing to show, so it never becomes a row.
            { author: "bo", submitted_at: "2026-08-21T11:00:00Z" },
          ],
          updated_at: "2026-08-21T10:05:00Z",
        },
        // No number is no handle to withdraw or link by: dropped rather than
        // drawn as a row nothing can be done to.
        { title: "A proposal with no number" },
      ],
      declined_proposals: [{ number: 2, status: "Declined" }],
      conflicts: [
        {
          id: "9f3c1ab0",
          path: "notes/a.md",
          kind: "EditEdit",
          detected_at: "2026-08-10T07:59:00Z",
        },
        // A bare path still counts above, but there is no id to open it by.
        "notes/b.md",
      ],
    });
    const status = await fetchSyncStatus("eng");

    // One list, open first: a row says which it is by the status it wears.
    expect(status.proposals.map((proposal) => proposal.number)).toEqual([4, 2]);
    expect(status.proposals.map((proposal) => proposal.status)).toEqual([
      "open",
      "declined",
    ]);
    expect(status.proposals[0]?.amendedUpstream).toBe(true);
    expect(status.proposals[0]?.reviewState).toBe("changes_requested");
    expect(status.proposals[0]?.feedback).toEqual([
      {
        author: "ana",
        body: "needs a source",
        path: "notes/a.md",
        line: 12,
        submittedAt: "2026-08-21T10:00:00Z",
        kind: "review_comment",
      },
    ]);
    expect(status.conflictList).toEqual([
      {
        id: "9f3c1ab0",
        path: "notes/a.md",
        kind: "EditEdit",
        detectedAt: "2026-08-10T07:59:00Z",
      },
    ]);
    // The counts still count everything, including what the lists dropped.
    expect(status.conflicts).toBe(2);
  });

  it("reads what a share would do, defaults included", async () => {
    apiMock.mockResolvedValueOnce({
      action: "update",
      effective_title: "Refine 2 engrams in kb",
      changes: [
        { path: "notes/a.md", kind: "modified" },
        // A change with no path is not a file anybody can be shown.
        { kind: "modified" },
      ],
      number: 4,
      url: "https://github.example/acme/kb/pull/4",
    });
    const plan = await fetchShareChanges("eng");

    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/sync/changes");
    expect(plan).toEqual({
      action: "update",
      effectiveTitle: "Refine 2 engrams in kb",
      changes: [{ path: "notes/a.md", kind: "modified" }],
      number: 4,
      url: "https://github.example/acme/kb/pull/4",
    });
  });

  it("shares a domain with the title and description it was given", async () => {
    apiMock.mockResolvedValueOnce({ outcome: "proposed", number: 4 });
    await shareDomain("team eng", { title: "From the UI" });

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/team%20eng/sync/share",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ title: "From the UI" }),
      }),
    );
  });

  it("withdraws a proposal by number, with the revert flag on the body", async () => {
    apiMock.mockResolvedValueOnce({
      number: 4,
      closed: true,
      status: "withdrawn",
      restored: ["notes/a.md"],
      deleted: [],
      skipped_diverged: ["notes/b.md"],
    });
    const receipt = await withdrawProposal("eng", 4, true);

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/sync/proposals/4/withdraw",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ revert: true }),
      }),
    );
    // The three file lists are read rather than passed through: they are how a
    // caller tells a withdraw that only closed a pull request from one that
    // moved files under every list on the screen.
    expect(receipt).toEqual({
      number: 4,
      closed: true,
      status: "withdrawn",
      restored: ["notes/a.md"],
      deleted: [],
      skippedDiverged: ["notes/b.md"],
    });
  });

  it("reads a withdraw that moved nothing as having moved nothing", async () => {
    // The lists a plain withdraw leaves out entirely. None of them becomes
    // undefined, because the caller counts them to decide what to re-read.
    apiMock.mockResolvedValueOnce({ number: 4, closed: true });
    const receipt = await withdrawProposal("eng", 4, false);

    expect(receipt).toEqual({
      number: 4,
      closed: true,
      status: "withdrawn",
      restored: [],
      deleted: [],
      skippedDiverged: [],
    });
  });

  it("reads one conflict's three sides, and keeps the id it asked by", async () => {
    apiMock.mockResolvedValueOnce({
      // The answer left the id out; the handle the resolve below is addressed
      // by is the one that was asked for.
      path: "notes/a.md",
      kind: "both_modified",
      base: "the shared start",
      local: "my version",
      upstream: null,
      note: "the upstream side is not UTF-8",
    });
    const detail = await fetchConflict("eng", "9f 3c");

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/sync/conflicts/9f%203c",
    );
    expect(detail).toEqual({
      id: "9f 3c",
      path: "notes/a.md",
      kind: "both_modified",
      base: "the shared start",
      local: "my version",
      upstream: null,
      note: "the upstream side is not UTF-8",
    });
  });

  it("resolves one conflict by id, with the merged content when there is any", async () => {
    apiMock.mockResolvedValueOnce({ resolved: "notes/a.md", remaining: 0 });
    await resolveConflict("eng", "9f3c1ab0", "merged", "the settled text");

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/sync/conflicts/9f3c1ab0/resolve",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          resolution: "merged",
          content: "the settled text",
        }),
      }),
    );

    apiMock.mockResolvedValueOnce({ resolved: "notes/a.md", remaining: 0 });
    await resolveConflict("eng", "9f3c1ab0", "mine");

    // No content to write, so no `content` key at all rather than a null the
    // server would have to read past.
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/sync/conflicts/9f3c1ab0/resolve",
      expect.objectContaining({
        body: JSON.stringify({ resolution: "mine" }),
      }),
    );
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
      newEntries: 0,
      collides: 0,
      written: 0,
      skipped: 0,
      invalid: 1,
      ignored: 0,
    });
  });

  it("carries the preview's own counters, and reads an absent one as none", async () => {
    const data = new ArrayBuffer(4);
    apiMock.mockResolvedValueOnce({
      ...PREVIEW_REPORT,
      new: 3,
      collides: 2,
    });
    const counted = await previewArchive("eng", data);
    // The wire key and the field a screen reads are spelled differently on
    // purpose, so the rename is part of what this pins.
    expect(counted.newEntries).toBe(3);
    expect(counted.collides).toBe(2);

    // An import's report tallies nothing under either key. Reading that as
    // none rather than as undefined is what keeps a counter line a number.
    const withoutCounters: Record<string, unknown> = { ...PREVIEW_REPORT };
    delete withoutCounters.new;
    delete withoutCounters.collides;
    apiMock.mockResolvedValueOnce(withoutCounters);
    const bare = await previewArchive("eng", data);
    expect(bare.newEntries).toBe(0);
    expect(bare.collides).toBe(0);
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
