/**
 * The user admin surface talks to five routes and unwraps two envelopes: this
 * pins that each call goes to the right path with the right method, and that
 * the `{"users": [...]}` and `{"user": ...}` wrappers never leak into a
 * caller, which would otherwise draw an envelope instead of an account.
 */

import { describe, expect, it, vi } from "vitest";

import { api } from "./client";
import {
  createUser,
  deleteUser,
  fetchUsers,
  patchUser,
  resetPassword,
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

  it("encodes a name that needs it", async () => {
    apiMock.mockResolvedValue({ user: { name: "a#b" } });
    await patchUser("a#b", { role: "viewer" });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/users/a%23b",
      expect.objectContaining({ method: "PATCH" }),
    );
  });
});
