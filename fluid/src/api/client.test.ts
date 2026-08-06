import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiProblem,
  api,
  encodePermalink,
  encodeSegment,
  engramPath,
  setCsrfToken,
} from "./client";

/** A response the wrapper should treat as a successful JSON body. */
function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/** A failure in the shape every route on this API answers with. */
function problemResponse(
  status: number,
  title: string,
  detail: string,
): Response {
  return new Response(
    JSON.stringify({ type: "about:blank", status, title, detail }),
    { status, headers: { "content-type": "application/problem+json" } },
  );
}

/** Install a fetch stub and hand back the spy the assertions read. */
function stubFetch(...responses: Response[]) {
  const queue = [...responses];
  const spy = vi.fn((_input: string | URL | Request, _init?: RequestInit) => {
    const next = queue.shift();
    if (!next) {
      throw new Error("fetch called more times than the test stubbed");
    }
    return Promise.resolve(next);
  });
  vi.stubGlobal("fetch", spy);
  return spy;
}

/** The headers of the nth recorded call, as a `Headers`. */
function headersOf(spy: ReturnType<typeof stubFetch>, call = 0): Headers {
  const init = spy.mock.calls[call]?.[1];
  return new Headers(init?.headers);
}

afterEach(() => {
  vi.unstubAllGlobals();
  setCsrfToken(null);
});

describe("api", () => {
  it("prefixes /api/v1 and sends the session cookie", async () => {
    const spy = stubFetch(jsonResponse({ domains: [] }));

    await api("/domains");

    expect(spy.mock.calls[0]?.[0]).toBe("/api/v1/domains");
    expect(spy.mock.calls[0]?.[1]?.credentials).toBe("same-origin");
  });

  it("parses a problem+json failure into ApiProblem", async () => {
    stubFetch(problemResponse(404, "not found", "no engram 'ghost' in 'eng'"));

    const failure = await api("/domains/eng/engrams/ghost").catch(
      (error: unknown) => error,
    );

    expect(failure).toBeInstanceOf(ApiProblem);
    const problem = failure as ApiProblem;
    expect(problem.status).toBe(404);
    expect(problem.title).toBe("not found");
    expect(problem.detail).toBe("no engram 'ghost' in 'eng'");
    expect(problem.message).toContain("no engram 'ghost' in 'eng'");
  });

  it("surfaces a 403 as an ApiProblem without retrying", async () => {
    const spy = stubFetch(
      problemResponse(403, "forbidden", "this account is disabled"),
    );

    const failure = await api("/auth/me").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).status).toBe(403);
    expect((failure as ApiProblem).detail).toBe("this account is disabled");
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("treats 400 and 422 alike: both carry the detail to show", async () => {
    for (const status of [400, 422]) {
      stubFetch(problemResponse(status, "rejected", "name must not be empty"));

      const failure = await api("/users", {
        method: "POST",
        body: JSON.stringify({ name: "" }),
      }).catch((error: unknown) => error);

      expect(failure).toBeInstanceOf(ApiProblem);
      expect((failure as ApiProblem).status).toBe(status);
      expect((failure as ApiProblem).detail).toBe("name must not be empty");
      vi.unstubAllGlobals();
    }
  });

  it("falls back to the transport status when a failure carries no problem body", async () => {
    stubFetch(new Response("<html>gateway</html>", { status: 502 }));

    const failure = await api("/domains").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).status).toBe(502);
    expect((failure as ApiProblem).detail).not.toBe("");
  });

  it("wraps a transport failure as an ApiProblem too", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.reject(new TypeError("Failed to fetch"))),
    );

    const failure = await api("/domains").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).status).toBe(0);
  });

  it("returns undefined for an empty 204 body", async () => {
    stubFetch(new Response(null, { status: 204 }));

    await expect(
      api("/users/ada", { method: "DELETE" }),
    ).resolves.toBeUndefined();
  });
});

describe("csrf", () => {
  it("attaches the token on an unsafe method but not on a safe one", async () => {
    setCsrfToken("9f2c1d7e4b6a8035");
    const spy = stubFetch(jsonResponse({ ok: true }), jsonResponse({}));

    await api("/auth/logout", { method: "POST" });
    await api("/domains");

    expect(headersOf(spy, 0).get("x-csrf-token")).toBe("9f2c1d7e4b6a8035");
    expect(headersOf(spy, 1).has("x-csrf-token")).toBe(false);
  });

  it("sends the JSON content type on unsafe methods and never a form one", async () => {
    const spy = stubFetch(jsonResponse({ ok: true }));

    await api("/users", { method: "POST", body: JSON.stringify({}) });

    expect(headersOf(spy).get("content-type")).toBe("application/json");
  });

  it("sends no token when there is none, which is the trusted-header case", async () => {
    const spy = stubFetch(jsonResponse({ ok: true }));

    await api("/auth/logout", { method: "POST" });

    expect(headersOf(spy).has("x-csrf-token")).toBe(false);
  });

  it("keeps the token the login response handed back", async () => {
    setCsrfToken("from-login");
    const spy = stubFetch(jsonResponse({ ok: true }));

    await api("/auth/logout", { method: "POST" });

    expect(headersOf(spy).get("x-csrf-token")).toBe("from-login");
  });
});

describe("path building", () => {
  it("encodes a segment, slashes included", () => {
    expect(encodeSegment("eng ops")).toBe("eng%20ops");
    expect(encodeSegment("a/b")).toBe("a%2Fb");
    expect(encodeSegment("a?b#c")).toBe("a%3Fb%23c");
  });

  it("keeps a permalink's internal slashes literal and encodes each segment", () => {
    expect(encodePermalink("notes/deep/gamma")).toBe("notes/deep/gamma");
    expect(encodePermalink("notes/deep dive/gamma")).toBe(
      "notes/deep%20dive/gamma",
    );
    expect(encodePermalink("notes/a?b/c#d")).toBe("notes/a%3Fb/c%23d");
  });

  it("builds an engram path a nested permalink survives", () => {
    expect(engramPath("eng", "notes/deep/gamma")).toBe(
      "/domains/eng/engrams/notes/deep/gamma",
    );
    expect(engramPath("eng ops", "notes/deep dive/gamma")).toBe(
      "/domains/eng%20ops/engrams/notes/deep%20dive/gamma",
    );
  });

  it("requests the built path unchanged", async () => {
    const spy = stubFetch(jsonResponse({}));

    await api(engramPath("eng", "notes/deep/gamma"));

    expect(spy.mock.calls[0]?.[0]).toBe(
      "/api/v1/domains/eng/engrams/notes/deep/gamma",
    );
  });
});
