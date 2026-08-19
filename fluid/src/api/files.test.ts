/**
 * The attachment surface: the two pure functions that decide where a file
 * lands, and the three calls that put it there.
 *
 * The naming rules are pinned hard, because the server refuses a path that
 * breaks them (`crates/core/src/attachment.rs`): a space, a parenthesis, a
 * `#`, a colon, a backslash, a leading dot or an extension that is not on the
 * allowlist. Every one of those refusals would reach an author as a failed
 * upload of a file they picked by hand, so the sanitizer is held to producing
 * names that pass rather than to looking tidy.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { CSRF_HEADER, setCsrfToken } from "./client";
import {
  ALLOWED_ATTACHMENT_EXTENSIONS,
  IMAGE_ATTACHMENT_EXTENSIONS,
  attachmentUrl,
  deleteAttachment,
  freeAttachmentPath,
  isAllowedAttachment,
  isImageAttachment,
  listAttachments,
  sanitizeAttachmentName,
  uploadAttachment,
} from "./files";

/** August 2026, built from local parts so no test depends on a time zone. */
const AUGUST = new Date(2026, 7, 3, 12, 0, 0);

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
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

afterEach(() => {
  vi.unstubAllGlobals();
  setCsrfToken(null);
});

describe("the allowlist", () => {
  it("mirrors the core extension table, markdown deliberately absent", () => {
    expect([...ALLOWED_ATTACHMENT_EXTENSIONS]).toEqual([
      "png",
      "jpg",
      "jpeg",
      "gif",
      "webp",
      "svg",
      "pdf",
      "pptx",
      "odp",
      "docx",
      "odt",
      "xlsx",
      "ods",
      "txt",
      "log",
      "csv",
      "json",
      "yaml",
      "yml",
      "toml",
      "xml",
    ]);
    expect(ALLOWED_ATTACHMENT_EXTENSIONS).not.toContain("md");
  });

  it("counts the six image types and nothing else as an image", () => {
    expect([...IMAGE_ATTACHMENT_EXTENSIONS]).toEqual([
      "png",
      "jpg",
      "jpeg",
      "gif",
      "webp",
      "svg",
    ]);
    expect(isImageAttachment("Shot.PNG")).toBe(true);
    expect(isImageAttachment("diagram.svg")).toBe(true);
    expect(isImageAttachment("deck.pdf")).toBe(false);
    expect(isImageAttachment("noextension")).toBe(false);
  });

  it("recognizes an allowed file whatever case its extension is written in", () => {
    expect(isAllowedAttachment("Deck.PPTX")).toBe(true);
    expect(isAllowedAttachment("notes.md")).toBe(false);
    expect(isAllowedAttachment("tool.exe")).toBe(false);
    expect(isAllowedAttachment("README")).toBe(false);
  });
});

describe("sanitizeAttachmentName", () => {
  it("lowercases, dashes the unsafe characters and keeps the extension", () => {
    expect(sanitizeAttachmentName("Q3 Deck (final).PDF")).toBe(
      "q3-deck-final.pdf",
    );
  });

  it("leaves a name that is already safe alone", () => {
    expect(sanitizeAttachmentName("flow-diagram.png")).toBe("flow-diagram.png");
  });

  it("never produces a hidden segment out of a dot-leading name", () => {
    expect(sanitizeAttachmentName(".hidden.png")).toBe("hidden.png");
  });

  it("falls back to a stem rather than producing a bare extension", () => {
    expect(sanitizeAttachmentName("###.png")).toBe("file.png");
  });

  it("collapses a run of unsafe characters into one dash", () => {
    expect(sanitizeAttachmentName("a   b___c.png")).toBe("a-b___c.png");
  });

  it("produces names the server's path rules accept", () => {
    const nasty = [
      "Q3 Deck (final).PDF",
      "C:report.xlsx",
      "notes#2.txt",
      "back\\slash.png",
      " padded .json",
      "Übersicht Plan.png",
      "..\\..\\escape.png",
    ];
    for (const name of nasty) {
      const safe = sanitizeAttachmentName(name);
      expect(safe).not.toMatch(/[ ():#\\]/);
      expect(safe.startsWith(".")).toBe(false);
      expect(safe).not.toBe("");
    }
  });
});

describe("freeAttachmentPath", () => {
  it("files an upload under the dated default folder, month zero padded", () => {
    expect(freeAttachmentPath("Q3 Deck (final).PDF", [], AUGUST)).toBe(
      "assets/2026/08/q3-deck-final.pdf",
    );
  });

  it("suffixes -2 and -3 against what that folder already holds", () => {
    expect(
      freeAttachmentPath(
        "Q3 Deck (final).PDF",
        ["assets/2026/08/q3-deck-final.pdf"],
        AUGUST,
      ),
    ).toBe("assets/2026/08/q3-deck-final-2.pdf");
    expect(
      freeAttachmentPath(
        "Q3 Deck (final).PDF",
        [
          "assets/2026/08/q3-deck-final.pdf",
          "assets/2026/08/q3-deck-final-2.pdf",
        ],
        AUGUST,
      ),
    ).toBe("assets/2026/08/q3-deck-final-3.pdf");
  });

  it("scopes the collision to the dated folder, so another month is no clash", () => {
    expect(
      freeAttachmentPath(
        "Q3 Deck (final).PDF",
        ["assets/2026/07/q3-deck-final.pdf"],
        AUGUST,
      ),
    ).toBe("assets/2026/08/q3-deck-final.pdf");
  });

  it("matches a stored path whatever case it was written in", () => {
    expect(
      freeAttachmentPath("Shot.PNG", ["assets/2026/08/SHOT.png"], AUGUST),
    ).toBe("assets/2026/08/shot-2.png");
  });
});

describe("attachmentUrl", () => {
  it("addresses the files route, keeping the asset path's own slashes", () => {
    expect(attachmentUrl("eng", "assets/2026/08/shot.png")).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
  });

  it("encodes a domain name that is not URL safe", () => {
    expect(attachmentUrl("my domain", "assets/a.png")).toBe(
      "/api/v1/domains/my%20domain/files/assets/a.png",
    );
  });
});

describe("uploadAttachment", () => {
  it("PUTs the raw bytes with the CSRF header and an explicit content type", async () => {
    setCsrfToken("token-1");
    const spy = stubFetch(
      jsonResponse({
        path: "assets/2026/08/shot.png",
        mime: "image/png",
        size: 3,
        sha256: "abc",
      }),
    );
    const file = new File([new Uint8Array([1, 2, 3])], "Shot.PNG", {
      type: "image/png",
    });

    const stored = await uploadAttachment(
      "eng",
      "assets/2026/08/shot.png",
      file,
    );

    expect(spy.mock.calls[0]?.[0]).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    const init = spy.mock.calls[0]?.[1];
    expect(init?.method).toBe("PUT");
    expect(new Headers(init?.headers).get(CSRF_HEADER)).toBe("token-1");
    expect(new Headers(init?.headers).get("Content-Type")).toBe("image/png");
    // The blob itself, never a JSON envelope around it.
    expect(init?.body).toBe(file);
    expect(stored).toEqual({
      path: "assets/2026/08/shot.png",
      mime: "image/png",
      size: 3,
      sha256: "abc",
    });
  });

  it("names a content type for a blob that carries none", async () => {
    const spy = stubFetch(
      jsonResponse({ path: "assets/a.txt", mime: "text/plain", size: 1 }),
    );

    await uploadAttachment("eng", "assets/a.txt", new Blob(["x"]));

    expect(
      new Headers(spy.mock.calls[0]?.[1]?.headers).get("Content-Type"),
    ).toBe("application/octet-stream");
  });
});

describe("deleteAttachment", () => {
  it("DELETEs the same path and accepts the empty answer", async () => {
    setCsrfToken("token-2");
    const spy = stubFetch(new Response(null, { status: 204 }));

    await deleteAttachment("eng", "assets/2026/08/shot.png");

    expect(spy.mock.calls[0]?.[0]).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    expect(spy.mock.calls[0]?.[1]?.method).toBe("DELETE");
    expect(new Headers(spy.mock.calls[0]?.[1]?.headers).get(CSRF_HEADER)).toBe(
      "token-2",
    );
  });
});

describe("listAttachments", () => {
  it("reads the rows and drops one missing its path", async () => {
    stubFetch(
      jsonResponse({
        attachments: [
          {
            path: "assets/2026/08/shot.png",
            mime: "image/png",
            size: 12,
            modified: "2026-08-18T09:12:00+00:00",
            sha256: "9f2a",
          },
          { mime: "image/png", size: 3 },
        ],
      }),
    );

    const rows = await listAttachments("eng");

    expect(rows).toEqual([
      {
        path: "assets/2026/08/shot.png",
        mime: "image/png",
        size: 12,
        modified: "2026-08-18T09:12:00+00:00",
        sha256: "9f2a",
      },
    ]);
  });
});
