/**
 * The user admin surface talks to five routes and unwraps two envelopes: this
 * pins that each call goes to the right path with the right method, and that
 * the `{"users": [...]}` and `{"user": ...}` wrappers never leak into a
 * caller, which would otherwise draw an envelope instead of an account.
 */

import { describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "./client";
import {
  bulkUsers,
  createUser,
  deleteUser,
  fetchUser,
  fetchUsers,
  fetchUsersPage,
  patchMyProfile,
  patchUser,
  resetPassword,
  setUserWrites,
  unlinkUserGithub,
} from "./users";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

describe("the users api module", () => {
  it("unwraps the listing envelope", async () => {
    apiMock.mockResolvedValueOnce({ users: [{ name: "ada" }] });
    const users = await fetchUsers();
    expect(apiMock).toHaveBeenCalledWith("/users");
    expect(users).toEqual([{ name: "ada" }]);
  });

  it("sends each mutation to its route with its method", async () => {
    apiMock.mockResolvedValue({ user: { name: "ada" } });
    await createUser({ name: "ada", role: "viewer", password: "pw" });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/users",
      expect.objectContaining({ method: "POST" }),
    );
    await patchUser("ada", { role: "editor" });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/users/ada",
      expect.objectContaining({ method: "PATCH" }),
    );
    await resetPassword("ada", "pw2");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/users/ada/password",
      expect.objectContaining({ method: "POST" }),
    );
    apiMock.mockResolvedValue(undefined);
    await deleteUser("ada");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/users/ada",
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  it("sends your own display name to /me and unwraps the account", async () => {
    apiMock.mockResolvedValue({ user: { name: "ada", display: "Ada L" } });
    const user = await patchMyProfile({ display: "Ada L" });
    expect(apiMock).toHaveBeenLastCalledWith("/me", {
      method: "PATCH",
      body: JSON.stringify({ display: "Ada L" }),
    });
    expect(user).toEqual({ name: "ada", display: "Ada L" });
  });

  it("encodes a name that needs it", async () => {
    apiMock.mockResolvedValue({ user: { name: "a#b" } });
    await patchUser("a#b", { role: "viewer" });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/users/a%23b",
      expect.objectContaining({ method: "PATCH" }),
    );
  });

  it("asks for one page with the filters set and nothing else", async () => {
    apiMock.mockResolvedValueOnce({ users: [{ name: "ada" }], total: 7 });
    const page = await fetchUsersPage({
      q: " ad ",
      role: "editor",
      state: null,
      kind: "token_only",
      github: false,
      seenAfter: null,
      seenBefore: "2026-05-06",
      sort: "last_seen",
      dir: "desc",
      page: 2,
      size: 25,
    });
    // "Until" a day includes it: the bound is the start of the next day.
    expect(apiMock).toHaveBeenCalledWith(
      "/users?q=ad&role=editor&kind=token_only&github=false&seen_before=2026-05-07T00%3A00%3A00Z&sort=last_seen&dir=desc&page=2&limit=25",
    );
    expect(page).toEqual({
      users: [{ name: "ada" }],
      total: 7,
      page: 2,
      size: 25,
    });
  });

  it("reads one account from its own route, and answers null for none", async () => {
    apiMock.mockResolvedValueOnce({
      user: { name: "a#b" },
      writes: ["eng"],
      drafts: [{ domain: "eng", count: 2 }],
    });
    expect(await fetchUser("a#b")).toEqual({
      user: { name: "a#b" },
      writes: ["eng"],
      drafts: [{ domain: "eng", count: 2 }],
      loaded: true,
    });
    expect(apiMock).toHaveBeenLastCalledWith("/users/a%23b");
    apiMock.mockResolvedValueOnce({ user: { name: "ada" } });
    expect(await fetchUser("ada")).toMatchObject({
      writes: null,
      drafts: null,
    });
    apiMock.mockRejectedValueOnce(new ApiProblem(404, "not found", "no"));
    expect(await fetchUser("ghost")).toBeNull();
    apiMock.mockRejectedValueOnce(new ApiProblem(500, "internal", "boom"));
    await expect(fetchUser("ada")).rejects.toThrow();
  });

  it("puts a whole write list, or null to remove it", async () => {
    apiMock.mockResolvedValueOnce({ name: "ada", writes: ["eng"] });
    expect(await setUserWrites("ada", ["eng"])).toEqual(["eng"]);
    expect(apiMock).toHaveBeenLastCalledWith("/users/ada/writes", {
      method: "PUT",
      body: JSON.stringify({ writes: ["eng"] }),
    });
    apiMock.mockResolvedValueOnce({ name: "ada" });
    expect(await setUserWrites("ada", null)).toBeNull();
    expect(apiMock).toHaveBeenLastCalledWith("/users/ada/writes", {
      method: "PUT",
      body: JSON.stringify({ writes: null }),
    });
  });

  it("sends a bulk change and answers its results", async () => {
    apiMock.mockResolvedValueOnce({ results: [{ name: "ada", done: true }] });
    const results = await bulkUsers({ names: ["ada"], action: "deactivate" });
    expect(apiMock).toHaveBeenLastCalledWith("/users/bulk", {
      method: "POST",
      body: JSON.stringify({ names: ["ada"], action: "deactivate" }),
    });
    expect(results).toEqual([{ name: "ada", done: true }]);
  });

  it("unlinks another account from GitHub and passes on a warning", async () => {
    apiMock.mockResolvedValueOnce({ linked: false, unlinked: true });
    expect(await unlinkUserGithub("a#b")).toBeNull();
    expect(apiMock).toHaveBeenLastCalledWith("/users/a%23b/github-link", {
      method: "DELETE",
    });
    apiMock.mockResolvedValueOnce({
      linked: false,
      unlinked: true,
      warning: "revoke it",
    });
    expect(await unlinkUserGithub("ada")).toBe("revoke it");
  });
});
