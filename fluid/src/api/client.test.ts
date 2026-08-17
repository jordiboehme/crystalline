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

/**
 * A response whose body cannot be read: the headers arrived, the stream broke
 * before the bytes did. `fetch` resolves such a response and only `text()`
 * rejects, which is what makes it worth pinning.
 */
function unreadableResponse(status = 200): Response {
  const response = new Response("{}", {
    status,
    headers: { "content-type": "application/json" },
  });
  Object.defineProperty(response, "text", {
    value: () => Promise.reject(new TypeError("network error")),
  });
  return response;
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

  it("parses a 200 that really is JSON", async () => {
    stubFetch(jsonResponse({ domains: ["eng"] }));

    await expect(api("/domains")).resolves.toEqual({ domains: ["eng"] });
  });

  it("refuses a 200 whose body is not JSON", async () => {
    // A captive portal or a misconfigured proxy answering with its own sign-in
    // page. It is a failed request wearing a success status, and resolving it
    // as undefined would move the failure to whatever reads the missing field.
    stubFetch(
      new Response("<html>sign in</html>", {
        status: 200,
        headers: { "content-type": "text/html; charset=utf-8" },
      }),
    );

    const failure = await api("/domains").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    const problem = failure as ApiProblem;
    expect(problem.status).toBe(200);
    expect(problem.title).toBe("unexpected response");
    expect(problem.detail).toContain("text/html");
  });

  it("refuses a 200 that announces JSON but is not", async () => {
    stubFetch(
      new Response("<html>sign in</html>", {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );

    const failure = await api("/domains").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).title).toBe("unexpected response");
    expect((failure as ApiProblem).detail).toContain("not valid JSON");
  });

  it("refuses a 200 whose body could not be read", async () => {
    // A stream that broke mid-read is not an empty body, and an empty body on
    // a listing route is a claim about the knowledge base: "no domains are
    // registered on this instance yet". The app must not state that fact
    // because a read failed, so this leaves here as the failure it is.
    stubFetch(unreadableResponse());

    const failure = await api("/domains").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    const problem = failure as ApiProblem;
    expect(problem.status).toBe(200);
    expect(problem.title).toBe("unexpected response");
    expect(problem.detail).toContain("could not be read");
  });

  it("keeps the transport status when a failure's body could not be read", async () => {
    stubFetch(unreadableResponse(503));

    const failure = await api("/domains").catch((error: unknown) => error);

    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).status).toBe(503);
    expect((failure as ApiProblem).detail).not.toBe("");
  });

  it("keeps a bodyless 200 as undefined rather than a failure", async () => {
    stubFetch(
      new Response("", {
        status: 200,
        headers: { "content-type": "text/html" },
      }),
    );

    await expect(api("/domains")).resolves.toBeUndefined();
  });
});

describe("problem extensions", () => {
  it("carries a problem body's extension members on the error", async () => {
    stubFetch(
      new Response(
        JSON.stringify({
          type: "about:blank",
          status: 412,
          title: "precondition failed",
          detail: "stale edit: engram changed since it was read",
          current_etag: '"abc123"',
          current_content: "---\ntitle: Alpha\n---\n\nTheirs.\n",
        }),
        {
          status: 412,
          headers: { "Content-Type": "application/problem+json" },
        },
      ),
    );
    const failure = await api("/domains/eng/engrams/alpha", {
      method: "PUT",
      body: "{}",
    }).catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(ApiProblem);
    const problem = failure as ApiProblem;
    expect(problem.status).toBe(412);
    expect(problem.extensions.current_etag).toBe('"abc123"');
    expect(problem.extensions.current_content).toContain("Theirs.");
    // The four standard members are fields, not extensions.
    expect(problem.extensions.title).toBeUndefined();
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

  it("escapes a literal percent so it cannot read as an escape", () => {
    // encodeURIComponent escapes the "%" itself, so a second pass over an
    // already encoded permalink is the failure mode, not this one.
    expect(encodePermalink("notes/100%/gamma")).toBe("notes/100%25/gamma");
    expect(encodePermalink("notes/%2Fnot-a-slash")).toBe(
      "notes/%252Fnot-a-slash",
    );
    expect(engramPath("eng", "notes/100%/gamma")).toBe(
      "/domains/eng/engrams/notes/100%25/gamma",
    );
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
