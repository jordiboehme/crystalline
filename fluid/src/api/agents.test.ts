/**
 * The agents and invites client layers: each call goes to the right path
 * with the right method and body.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  AGENTS_KEY,
  createAgent,
  fetchAgents,
  issueAgentToken,
  removeAgent,
  revokeAgentToken,
} from "./agents";
import { api } from "./client";
import {
  INVITES_KEY,
  createInvite,
  fetchInvites,
  revokeInvite,
} from "./invites";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

beforeEach(() => {
  apiMock.mockReset();
  apiMock.mockResolvedValue(undefined);
});

describe("the agents api module", () => {
  it("carries a stable cache key", () => {
    expect(AGENTS_KEY).toEqual(["agents"]);
  });

  it("reaches every agents route", async () => {
    await fetchAgents();
    expect(apiMock).toHaveBeenLastCalledWith("/agents");
    await createAgent({ name: "nightly", loops: "both" });
    expect(apiMock).toHaveBeenLastCalledWith("/agents", {
      method: "POST",
      body: JSON.stringify({ name: "nightly", loops: "both" }),
    });
    await issueAgentToken("night ly", { label: "two", expires_days: 30 });
    expect(apiMock).toHaveBeenLastCalledWith("/agents/night%20ly/tokens", {
      method: "POST",
      body: JSON.stringify({ label: "two", expires_days: 30 }),
    });
    await revokeAgentToken("nightly", 7);
    expect(apiMock).toHaveBeenLastCalledWith("/agents/nightly/tokens/7", {
      method: "DELETE",
    });
    await removeAgent("nightly");
    expect(apiMock).toHaveBeenLastCalledWith("/agents/nightly", {
      method: "DELETE",
    });
  });
});

describe("the invites api module", () => {
  it("carries a stable cache key", () => {
    expect(INVITES_KEY).toEqual(["invites"]);
  });

  it("reaches every invites route", async () => {
    await fetchInvites();
    expect(apiMock).toHaveBeenLastCalledWith("/invites");
    await createInvite({ role: "viewer", expires_in_days: 7 });
    expect(apiMock).toHaveBeenLastCalledWith("/invites", {
      method: "POST",
      body: JSON.stringify({ role: "viewer", expires_in_days: 7 }),
    });
    await revokeInvite(4);
    expect(apiMock).toHaveBeenLastCalledWith("/invites/4", {
      method: "DELETE",
    });
  });
});
