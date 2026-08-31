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
  MY_GITHUB_IDENTITY_KEY,
  SYNC_SUMMARY_KEY,
  archiveDownloadUrl,
  connectMyGithubIdentityToken,
  createDomain,
  disconnectGithub,
  disconnectMyGithubIdentity,
  fetchConflict,
  fetchGithubStatus,
  fetchMyGithubIdentity,
  fetchShareChanges,
  fetchSyncStatus,
  fetchSyncSummary,
  importArchive,
  previewArchive,
  readGithubStatus,
  readMyGithubIdentity,
  readStackPlacement,
  resolveConflict,
  shareDomain,
  sharePlanKey,
  startGithubConnect,
  startMyGithubIdentityDevice,
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

  it("reads my own GitHub identity out of its wire spelling", async () => {
    apiMock.mockResolvedValueOnce({
      account: "ada",
      connected: false,
      login: null,
      connected_at: null,
      token_store: null,
      pending: {
        user_code: "ABCD-1234",
        verification_url: "https://github.example/device",
        expires_in_secs: 900,
      },
      error: null,
    });
    const identity = await fetchMyGithubIdentity();

    // The caller's own, so no path segment names it: the session does.
    expect(apiMock).toHaveBeenLastCalledWith("/me/github-identity");
    expect(identity).toEqual({
      account: "ada",
      connected: false,
      login: null,
      connectedAt: null,
      tokenStore: null,
      pending: {
        userCode: "ABCD-1234",
        verificationUrl: "https://github.example/device",
        expiresInSecs: 900,
      },
      error: null,
    });
  });

  it("reads a connected identity, and a report that left fields out", () => {
    expect(
      readMyGithubIdentity({
        account: "ada",
        connected: true,
        login: "octo",
        connected_at: "2026-08-29T09:12:44Z",
        token_store: "keyring",
        pending: null,
        error: "the code expired before it was confirmed",
      }),
    ).toEqual({
      account: "ada",
      connected: true,
      login: "octo",
      connectedAt: "2026-08-29T09:12:44Z",
      tokenStore: "keyring",
      pending: null,
      error: "the code expired before it was confirmed",
    });

    // This one is the card's poll as well as its read, so a field that is
    // briefly missing leaves the card saying "not connected" rather than
    // throwing inside a render. Nonsense reads the same way.
    const bare = {
      account: "",
      connected: false,
      login: null,
      connectedAt: null,
      tokenStore: null,
      pending: null,
      error: null,
    };
    expect(readMyGithubIdentity({})).toEqual(bare);
    expect(readMyGithubIdentity("nonsense")).toEqual(bare);
    // A code with nowhere to type it is not something to put in front of
    // somebody, so half a pending block is no pending block.
    expect(
      readMyGithubIdentity({ pending: { user_code: "ABCD-1234" } }).pending,
    ).toBeNull();
  });

  it("starts my own device flow on its own route, and files it outside the domain family", async () => {
    apiMock.mockResolvedValueOnce({
      account: "ada",
      connected: false,
      pending: {
        user_code: "ABCD-1234",
        verification_url: "https://github.example/device",
        expires_in_secs: 900,
      },
    });
    const identity = await startMyGithubIdentityDevice();

    expect(apiMock).toHaveBeenLastCalledWith(
      "/me/github-identity/connect",
      expect.objectContaining({ method: "POST" }),
    );
    expect(identity.pending?.userCode).toBe("ABCD-1234");
    // A personal identity is nobody's domain content, so no bulk domain
    // invalidation reaches it.
    expect(MY_GITHUB_IDENTITY_KEY).toEqual(["me-github-identity"]);
    expect(MY_GITHUB_IDENTITY_KEY[0]).not.toBe(syncStatusKey("eng")[0]);
  });

  it("puts my token and keeps no copy of it", async () => {
    apiMock.mockResolvedValueOnce({
      account: "ada",
      connected: true,
      login: "octo",
      connected_at: "2026-08-29T09:12:44Z",
      token_store: "keyring",
    });
    const identity = await connectMyGithubIdentityToken("ghp_secret");

    // PUT rather than POST: this replaces the caller's one identity, so
    // re-pasting a token is the same request twice with the same result.
    expect(apiMock).toHaveBeenLastCalledWith(
      "/me/github-identity/token",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ token: "ghp_secret" }),
      }),
    );
    expect(JSON.stringify(identity)).not.toContain("ghp_secret");
  });

  it("forgets my own credential with a DELETE", async () => {
    apiMock.mockResolvedValueOnce({ account: "ada", connected: false });
    const identity = await disconnectMyGithubIdentity();

    expect(apiMock).toHaveBeenLastCalledWith(
      "/me/github-identity",
      expect.objectContaining({ method: "DELETE" }),
    );
    expect(identity.connected).toBe(false);
    expect(identity.login).toBeNull();
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
      // The report said nothing about whose work it is, which is not the same
      // as saying none of it is this account's.
      ownedChanges: null,
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
          // Nobody is named for a record shared before the login was
          // recorded, rather than a login invented for it.
          authorLogin: null,
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
          authorLogin: null,
        },
      ],
      conflictList: [],
      // The identity keys the connection block carries on an instance that
      // has them: a report with no block says nothing about either rather
      // than defaulting to a mode nobody set.
      shareIdentity: null,
      ownerIdentity: null,
      // The four chain keys the route always sends, read out of a report that
      // sent none of them: nothing is stacked, nothing is wedged and neither
      // debt is outstanding. Quiet defaults rather than holes, because every
      // one of them is drawn as a badge or a banner when it is set.
      stackNumber: null,
      stackWedged: [],
      repairPending: false,
      stackLinkPending: false,
    });
    expect(syncStatusKey("eng")).toEqual(["domains", "eng", "sync"]);
  });

  it("reads where the domain's chain of stacked proposals stands", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      open_proposals: [{ number: 7 }, { number: 9 }],
      stack_number: 42,
      // A declined layer still carrying open layers above it, by number: that
      // number is what a reader withdraws or shares against.
      stack_wedged: [3, "nonsense"],
      repair_pending: true,
      stack_link_pending: true,
    });
    const status = await fetchSyncStatus("eng");

    expect(status.stackNumber).toBe(42);
    // Only the numbers: a wedged entry that is not one is nothing a screen can
    // address a proposal by.
    expect(status.stackWedged).toEqual([3]);
    expect(status.repairPending).toBe(true);
    expect(status.stackLinkPending).toBe(true);
  });

  it("reads a chain whose linking call has not landed as pending, not as stack null", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      open_proposals: [{ number: 7 }, { number: 9 }],
      // Every layer exists; they are simply not grouped on the forge yet.
      stack_number: null,
      stack_wedged: [],
      repair_pending: false,
      stack_link_pending: true,
    });
    const status = await fetchSyncStatus("eng");

    expect(status.stackNumber).toBeNull();
    expect(status.stackLinkPending).toBe(true);
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

  it("reads how much of the waiting work is this session's own", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      local_changes: 5,
      owned_changes: 2,
      open_proposals: [],
    });
    const status = await fetchSyncStatus("eng");

    expect(status.localChanges).toBe(5);
    expect(status.ownedChanges).toBe(2);

    // Zero is an answer and stays one; a non-number is not an answer at all,
    // and folding it to zero would tell a reader none of the waiting work is
    // theirs on the word of a report that never said so.
    apiMock.mockResolvedValueOnce({
      repo: "acme/kb",
      local_changes: 5,
      owned_changes: 0,
      open_proposals: [],
    });
    expect((await fetchSyncStatus("eng")).ownedChanges).toBe(0);

    apiMock.mockResolvedValueOnce({
      repo: "acme/kb",
      local_changes: 5,
      owned_changes: "two",
      open_proposals: [],
    });
    expect((await fetchSyncStatus("eng")).ownedChanges).toBeNull();
  });

  it("reads the owned count on a summary row the same way", async () => {
    apiMock.mockResolvedValueOnce({
      domains: [
        { domain: "eng", local_changes: 5, owned_changes: 2 },
        { domain: "ops", local_changes: 3 },
      ],
    });
    const summary = await fetchSyncSummary();

    expect(summary.domains.map((entry) => entry.ownedChanges)).toEqual([
      2,
      null,
    ]);
  });

  it("reads which identity this instance shares as, and the owner's slot", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      open_proposals: [],
      connection: {
        connected: true,
        user: "octo",
        token_store: "keychain",
        share_identity: "personal",
        // The MACHINE owner's slot, which is what a CLI or stdio share
        // resolves. Read because the report sends it; never mistaken for the
        // browser's own acting identity, which is the session's.
        owner_identity: { account: "owner", connected: false, user: null },
      },
    });
    const status = await fetchSyncStatus("eng");

    expect(status.shareIdentity).toBe("personal");
    expect(status.ownerIdentity).toEqual({
      account: "owner",
      connected: false,
      user: null,
    });
  });

  it("reads an instance-mode report as saying nothing about an owner slot", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      repo: "acme/kb",
      open_proposals: [],
      // The default mode, where there is no personal slot in play at all: the
      // block carries the mode and nothing else.
      connection: { connected: true, share_identity: "instance" },
    });
    const status = await fetchSyncStatus("eng");

    expect(status.shareIdentity).toBe("instance");
    expect(status.ownerIdentity).toBeNull();
  });

  it("reads identity keys of nonsense as no answer rather than as a mode", async () => {
    apiMock.mockResolvedValueOnce({
      repo: "acme/kb",
      connection: { share_identity: 7, owner_identity: "nonsense" },
    });
    const status = await fetchSyncStatus("eng");

    // Only a word is a mode, and only a record is a slot: a screen that read
    // either of these as `personal` would swap a working button for a connect
    // link on an instance that never asked for one.
    expect(status.shareIdentity).toBeNull();
    expect(status.ownerIdentity).toBeNull();
  });

  it("reads the login a proposal was shared as, where the record names one", async () => {
    apiMock.mockResolvedValueOnce({
      repo: "acme/kb",
      open_proposals: [
        { number: 7, author_login: "octo" },
        // Shared before this was recorded, and a record that carries a
        // non-string where the login goes: both are "nobody named".
        { number: 9 },
        { number: 11, author_login: 42 },
      ],
    });
    const status = await fetchSyncStatus("eng");

    expect(status.proposals.map((proposal) => proposal.authorLogin)).toEqual([
      "octo",
      null,
      null,
    ]);
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

  it("reads the instance-wide summary, and files it outside the domain family", async () => {
    apiMock.mockResolvedValueOnce({
      connection: { connected: true, user: "octo", token_store: "keychain" },
      domains: [
        {
          domain: "eng",
          mode: "github",
          repo: "acme/kb",
          branch: "main",
          last_checked: "2026-08-10T08:00:00Z",
          local_changes: 2,
          open_proposals: 1,
          declined_proposals: 0,
          conflicts: 0,
        },
        // No name is no domain to share into, so it never becomes a row: the
        // name is the handle every screen reading this addresses a share by.
        { local_changes: 5 },
      ],
      errors: [],
    });
    const summary = await fetchSyncSummary();

    expect(apiMock).toHaveBeenLastCalledWith("/sync");
    // Only what a share action needs. The entry carries a repository, a branch
    // and a last-checked instant too, and none of them is read here: the card
    // that draws those reads the per-domain report.
    expect(summary).toEqual({
      connected: true,
      domains: [
        {
          domain: "eng",
          localChanges: 2,
          ownedChanges: null,
          openProposals: 1,
          declinedProposals: 0,
          conflicts: 0,
          // Chain health rides along with the row, because a picker has to
          // know which domains it can actually offer; where a domain sits IN
          // its chain stays on the per-domain report.
          stackWedged: [],
          repairPending: false,
          stackLinkPending: false,
        },
      ],
    });
    // And the key it is cached under, deliberately outside the `["domains"]`
    // family: reading this route probes GitHub, so a bulk domain invalidation
    // must never reach it.
    expect(SYNC_SUMMARY_KEY).toEqual(["sync-summary"]);
    expect(SYNC_SUMMARY_KEY[0]).not.toBe(syncStatusKey("eng")[0]);
  });

  it("reads a summary that counted nothing as zero, and no block as no answer", async () => {
    apiMock.mockResolvedValueOnce({ domains: [{ domain: "eng" }] });
    const summary = await fetchSyncSummary();

    // A count the report left out is none rather than a hole, and a report
    // with no connection block says nothing about the credential rather than
    // telling somebody to connect what is already connected.
    expect(summary.connected).toBeNull();
    expect(summary.domains).toEqual([
      {
        domain: "eng",
        localChanges: 0,
        ownedChanges: null,
        openProposals: 0,
        declinedProposals: 0,
        conflicts: 0,
        stackWedged: [],
        repairPending: false,
        stackLinkPending: false,
      },
    ]);
  });

  it("reads a summary row's chain health, so a picker can badge it", async () => {
    apiMock.mockResolvedValueOnce({
      domains: [
        {
          domain: "eng",
          local_changes: 2,
          stack_wedged: [3],
          repair_pending: true,
          stack_link_pending: false,
        },
      ],
    });
    const summary = await fetchSyncSummary();

    expect(summary.domains[0]?.stackWedged).toEqual([3]);
    expect(summary.domains[0]?.repairPending).toBe(true);
    expect(summary.domains[0]?.stackLinkPending).toBe(false);
  });

  it("reads an instance with no credential on file as not connected", async () => {
    // The summary reports a missing connection rather than refusing over it,
    // so `false` is an answer a share action has to be able to act on.
    apiMock.mockResolvedValueOnce({
      connection: { connected: false },
      domains: [],
    });

    expect((await fetchSyncSummary()).connected).toBe(false);
  });

  it("reads what a share would do, defaults included", async () => {
    apiMock.mockResolvedValueOnce({
      action: "update",
      effective_title: "Refine 2 engrams in kb",
      changes: [
        { path: "notes/a.md", kind: "modified", last_author: "human:ada" },
        // A change nobody is named for: an engram edited outside the engine,
        // or an older server that names nobody at all.
        { path: "notes/b.md", kind: "modified" },
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
      changes: [
        { path: "notes/a.md", kind: "modified", lastAuthor: "human:ada" },
        { path: "notes/b.md", kind: "modified", lastAuthor: null },
      ],
      number: 4,
      url: "https://github.example/acme/kb/pull/4",
      // An update carries no conflict count, and none is invented for it.
      count: null,
      // Nor any of the three fields the two stacked plans carry.
      topNumber: null,
      topTitle: null,
      layersAbove: null,
    });
    // And the key it is cached under, which is deliberately not one of the
    // `["domains", ...]` keys every other read of a domain is filed under:
    // reading this route pulls the origin, so a bulk domain invalidation must
    // never reach it.
    expect(sharePlanKey("eng")).toEqual(["share-plan", "eng"]);
    expect(sharePlanKey("eng")[0]).not.toBe(syncStatusKey("eng")[0]);
  });

  it("reads the conflict count off a plan that is waiting on them", async () => {
    apiMock.mockResolvedValueOnce({
      action: "conflicts_pending",
      count: 2,
      effective_title: "",
      changes: [],
    });
    const plan = await fetchShareChanges("eng");

    // The one number that says how much work settling them is, before the
    // screen that settles them is opened.
    expect(plan.count).toBe(2);
    expect(plan.action).toBe("conflicts_pending");
  });

  it("reads the layer a stack would sit on, and the layers an amend rebuilds", async () => {
    apiMock.mockResolvedValueOnce({
      action: "stack",
      effective_title: "Refine 1 engram in kb",
      changes: [],
      top_number: 4,
      top_title: "Refine 2 engrams in kb",
    });
    const stack = await fetchShareChanges("eng");

    expect(stack.action).toBe("stack");
    expect(stack.topNumber).toBe(4);
    expect(stack.topTitle).toBe("Refine 2 engrams in kb");

    apiMock.mockResolvedValueOnce({
      action: "amend",
      effective_title: "Refine 1 engram in kb",
      changes: [],
      number: 4,
      url: "https://github.example/acme/kb/pull/4",
      layers_above: 2,
    });
    const amend = await fetchShareChanges("eng");

    // How much work the amend would rebuild, which is the whole difference
    // between amending the top layer and amending one under it.
    expect(amend.number).toBe(4);
    expect(amend.layersAbove).toBe(2);
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

  it("names the open layer to amend on the share body", async () => {
    apiMock.mockResolvedValueOnce({
      outcome: "updated",
      proposal: { number: 4 },
    });
    await shareDomain("eng", { proposal: 4 });

    // The one field that turns a share from "stack a new layer" into "amend
    // this one", and it travels only when somebody chose a layer.
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/sync/share",
      expect.objectContaining({ body: JSON.stringify({ proposal: 4 }) }),
    );
  });

  it("reads where a shared proposal sits in its chain, position first", () => {
    // The position is the gate and the number is named only when there is one:
    // a chain whose linking call failed carries real positions with a null
    // number, and "stack #null" would be worse than saying nothing.
    expect(
      readStackPlacement({ stack_number: 42, stack_position: [2, 3] }),
    ).toEqual({ stackNumber: 42, stackPosition: [2, 3] });
    expect(
      readStackPlacement({ stack_number: null, stack_position: [2, 3] }),
    ).toEqual({ stackNumber: null, stackPosition: [2, 3] });
    // Off the stacked path both are null rather than absent, and a position
    // that is not a pair of numbers is no position at all.
    expect(
      readStackPlacement({ stack_number: null, stack_position: null }),
    ).toEqual({ stackNumber: null, stackPosition: null });
    expect(
      readStackPlacement({ stack_position: [2] }).stackPosition,
    ).toBeNull();
    expect(readStackPlacement("nonsense")).toEqual({
      stackNumber: null,
      stackPosition: null,
    });
  });

  it("withdraws a proposal by number, with the revert flag on the body", async () => {
    apiMock.mockResolvedValueOnce({
      number: 4,
      closed: true,
      status: "withdrawn",
      restored: ["notes/a.md"],
      deleted: [],
      skipped_diverged: ["notes/b.md"],
      // The second reason a revert leaves a file alone: no reachable copy of
      // what it looked like before the share.
      skipped_reverts: ["notes/c.md"],
      repaired: true,
      // The rebuild allocated a new number; the old one never comes back.
      restacked: 43,
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
      skippedReverts: ["notes/c.md"],
      repaired: true,
      restacked: 43,
    });
  });

  it("reads a repair that found too few survivors to be a stack", async () => {
    apiMock.mockResolvedValueOnce({
      number: 4,
      closed: true,
      repaired: true,
      // Null covers both "no repair happened" and "the survivors no longer
      // make a chain", so it is read together with `repaired` rather than
      // alone.
      restacked: null,
    });
    const receipt = await withdrawProposal("eng", 4, false);

    expect(receipt.repaired).toBe(true);
    expect(receipt.restacked).toBeNull();
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
      skippedReverts: [],
      repaired: false,
      restacked: null,
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
