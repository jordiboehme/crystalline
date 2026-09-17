/**
 * The MCP token client layer talks to four routes: this pins that each call
 * goes to the right path with the right method and body, and that revoke's
 * empty reply never becomes anything a caller has to unwrap.
 */

import { describe, expect, it, vi } from "vitest";

import { api } from "./client";
import {
  MCP_TOKENS_KEY,
  fetchMcpTokens,
  issueMcpToken,
  revokeMcpToken,
  rotateMcpToken,
} from "./mcpTokens";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

describe("the mcp tokens api module", () => {
  it("carries a stable cache key", () => {
    expect(MCP_TOKENS_KEY).toEqual(["me-mcp-tokens"]);
  });

  it("lists the caller's own tokens", async () => {
    apiMock.mockResolvedValueOnce([{ id: 1, label: "laptop" }]);
    const tokens = await fetchMcpTokens();
    expect(apiMock).toHaveBeenCalledWith("/me/mcp-tokens");
    expect(tokens).toEqual([{ id: 1, label: "laptop" }]);
  });

  it("issues a token with its label in the body", async () => {
    apiMock.mockResolvedValueOnce({ id: 3, label: "laptop", token: "cmt_x" });
    const issued = await issueMcpToken("laptop");
    expect(apiMock).toHaveBeenCalledWith("/me/mcp-tokens", {
      method: "POST",
      body: JSON.stringify({ label: "laptop" }),
    });
    expect(issued).toEqual({ id: 3, label: "laptop", token: "cmt_x" });
  });

  it("rotates a token by id", async () => {
    apiMock.mockResolvedValueOnce({ id: 3, label: "laptop", token: "cmt_y" });
    await rotateMcpToken(3);
    expect(apiMock).toHaveBeenCalledWith("/me/mcp-tokens/3/rotate", {
      method: "POST",
    });
  });

  it("revokes a token by id and returns nothing", async () => {
    apiMock.mockResolvedValueOnce(undefined);
    const result = await revokeMcpToken(3);
    expect(apiMock).toHaveBeenCalledWith("/me/mcp-tokens/3", {
      method: "DELETE",
    });
    expect(result).toBeUndefined();
  });
});
