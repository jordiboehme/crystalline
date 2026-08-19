/**
 * The upload flow behind paste, drop and the attach button: what is refused
 * before a byte leaves the browser, what the buffer gets afterwards, and what
 * happens to the document when an upload fails (nothing, which is the point -
 * a reference to a file that was never stored is a dangling reference the
 * maintenance sweep would then have to flag).
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiProblem } from "../api/client";
import { listAttachments, uploadAttachment } from "../api/files";
import {
  attachmentMarkdown,
  attachmentUploads,
  refuseAttachment,
  transferFiles,
  uploadAttachments,
} from "./attachments";
import { frontmatterFold } from "./frontmatterFold";

vi.mock("../api/files", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/files")>();
  return { ...actual, listAttachments: vi.fn(), uploadAttachment: vi.fn() };
});

const listMock = vi.mocked(listAttachments);
const uploadMock = vi.mocked(uploadAttachment);

/** A file of a given name and size, with no bytes actually allocated. */
function fileOf(name: string, size = 8): File {
  const file = new File(["x"], name);
  Object.defineProperty(file, "size", { value: size });
  return file;
}

let view: EditorView | null = null;

afterEach(() => {
  view?.destroy();
  view = null;
  vi.clearAllMocks();
});

/** A live buffer with the caret on its one line. */
function mount(doc = "Body line\n"): EditorView {
  view = new EditorView({ state: EditorState.create({ doc }) });
  view.dispatch({ selection: { anchor: doc.indexOf("\n") } });
  return view;
}

describe("refuseAttachment", () => {
  it("passes an allowed file under the ceiling", () => {
    expect(refuseAttachment(fileOf("shot.png"))).toBeNull();
  });

  it("judges the name the sanitizer would store, padding and all", () => {
    // The pre-check and the path builder have to agree about what the file is
    // called, or a name a trailing space is refused for is a name that would
    // have been stored perfectly legally.
    expect(refuseAttachment(fileOf("shot.png "))).toBeNull();
  });

  it("refuses an extension that is not on the allowlist, naming the file", () => {
    const refusal = refuseAttachment(fileOf("tool.exe"));
    expect(refusal).toContain("tool.exe");
    expect(refusal).toMatch(/png/);
  });

  it("refuses a file past 10 MiB before it is uploaded", () => {
    const refusal = refuseAttachment(fileOf("huge.png", 10 * 1024 * 1024 + 1));
    expect(refusal).toContain("huge.png");
    expect(refusal).toContain("10 MiB");
  });
});

describe("attachmentMarkdown", () => {
  it("embeds an image and links everything else", () => {
    expect(attachmentMarkdown("Shot.PNG", "assets/2026/08/shot.png")).toBe(
      "![Shot.PNG](assets/2026/08/shot.png)",
    );
    expect(attachmentMarkdown("diagram.svg", "assets/a/diagram.svg")).toBe(
      "![diagram.svg](assets/a/diagram.svg)",
    );
    expect(attachmentMarkdown("Q3.pdf", "assets/2026/08/q3.pdf")).toBe(
      "[Q3.pdf](assets/2026/08/q3.pdf)",
    );
  });

  it("keeps a bracket in the name out of the link text", () => {
    expect(attachmentMarkdown("a[b].pdf", "assets/a-b.pdf")).toBe(
      "[a b .pdf](assets/a-b.pdf)",
    );
  });
});

describe("transferFiles", () => {
  it("reads the files off a clipboard or drag payload, and nothing off text", () => {
    const file = fileOf("shot.png");
    expect(transferFiles({ files: [file] } as unknown as DataTransfer)).toEqual(
      [file],
    );
    expect(transferFiles({ files: [] } as unknown as DataTransfer)).toEqual([]);
    expect(transferFiles(null)).toEqual([]);
  });
});

describe("attachmentUploads", () => {
  /**
   * A clipboard or drag event carrying these files. `getData` is there for
   * the editor's own handler, which reads the text payload of anything this
   * module hands back to it.
   */
  function transferEvent(
    type: "paste" | "drop" | "dragover",
    files: File[],
  ): Event {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(
      event,
      type === "paste" ? "clipboardData" : "dataTransfer",
      { value: { files, types: [], getData: () => "" } },
    );
    return event;
  }

  it("takes the files off a paste and stops the browser pasting them too", () => {
    const seen: File[][] = [];
    view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [
          attachmentUploads((files) => {
            seen.push(files);
          }),
        ],
      }),
    });
    const file = fileOf("shot.png");

    const event = transferEvent("paste", [file]);
    view.contentDOM.dispatchEvent(event);

    expect(seen).toEqual([[file]]);
    expect(event.defaultPrevented).toBe(true);
  });

  it("leaves a paste with no files to the editor", () => {
    const seen: File[][] = [];
    view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [
          attachmentUploads((files) => {
            seen.push(files);
          }),
        ],
      }),
    });

    const event = transferEvent("paste", []);
    view.contentDOM.dispatchEvent(event);

    // What the editor then does with a text paste is the editor's business -
    // it cancels the event itself - so the assertion is that this module did
    // not take it, not that nobody did.
    expect(seen).toEqual([]);
  });

  it("takes the files off a drop", () => {
    const seen: File[][] = [];
    view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [
          attachmentUploads((files) => {
            seen.push(files);
          }),
        ],
      }),
    });
    const file = fileOf("shot.png");

    const event = transferEvent("drop", [file]);
    view.contentDOM.dispatchEvent(event);

    expect(seen).toEqual([[file]]);
    expect(event.defaultPrevented).toBe(true);
  });

  it("leaves a drop with no files to the editor", () => {
    const seen: File[][] = [];
    view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [
          attachmentUploads((files) => {
            seen.push(files);
          }),
        ],
      }),
    });

    view.contentDOM.dispatchEvent(transferEvent("drop", []));

    expect(seen).toEqual([]);
  });

  it("cancels a file dragover and only a file dragover", () => {
    // The one that is invisible until it breaks: a browser fires no drop at
    // all unless the dragover before it was cancelled, so a regression here
    // kills drag and drop with every other test still green.
    view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [attachmentUploads(() => undefined)],
      }),
    });

    const withFiles = transferEvent("dragover", [fileOf("shot.png")]);
    const withoutFiles = transferEvent("dragover", []);
    view.contentDOM.dispatchEvent(withFiles);
    view.contentDOM.dispatchEvent(withoutFiles);

    expect(withFiles.defaultPrevented).toBe(true);
    expect(withoutFiles.defaultPrevented).toBe(false);
  });
});

describe("uploadAttachments", () => {
  it("uploads under the dated folder and inserts one line per file", async () => {
    const target = mount();
    listMock.mockResolvedValueOnce([]);
    uploadMock.mockImplementation((_domain, path) =>
      Promise.resolve({ path, mime: "image/png", size: 8, sha256: "a" }),
    );
    const errors: (string | null)[] = [];

    const inserted = await uploadAttachments({
      domain: "eng",
      files: [fileOf("Q3 Deck (final).PDF"), fileOf("Shot.PNG")],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(inserted).toBe(2);
    expect(uploadMock.mock.calls.map((call) => call[1])).toEqual([
      "assets/2026/08/q3-deck-final.pdf",
      "assets/2026/08/shot.png",
    ]);
    expect(target.state.doc.toString()).toContain(
      "[Q3 Deck (final).PDF](assets/2026/08/q3-deck-final.pdf)\n" +
        "![Shot.PNG](assets/2026/08/shot.png)",
    );
    // The one entry is the clear the batch opens with.
    expect(errors).toEqual([null]);
  });

  it("suffixes against the domain's existing files and against this batch", async () => {
    const target = mount();
    listMock.mockResolvedValueOnce([
      {
        path: "assets/2026/08/shot.png",
        mime: "image/png",
        size: 1,
        modified: "2026-08-01T00:00:00+00:00",
        sha256: "a",
      },
    ]);
    uploadMock.mockImplementation((_domain, path) =>
      Promise.resolve({ path, mime: "image/png", size: 8, sha256: "a" }),
    );

    await uploadAttachments({
      domain: "eng",
      files: [fileOf("shot.png"), fileOf("shot.png")],
      view: target,
      onError: () => undefined,
      now: new Date(2026, 7, 3),
    });

    expect(uploadMock.mock.calls.map((call) => call[1])).toEqual([
      "assets/2026/08/shot-2.png",
      "assets/2026/08/shot-3.png",
    ]);
  });

  it("reports a refused file and never uploads it", async () => {
    const target = mount();
    listMock.mockResolvedValueOnce([]);
    const errors: (string | null)[] = [];

    const inserted = await uploadAttachments({
      domain: "eng",
      files: [fileOf("tool.exe")],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(inserted).toBe(0);
    expect(uploadMock).not.toHaveBeenCalled();
    expect(errors.at(-1)).toContain("tool.exe");
    expect(target.state.doc.toString()).toBe("Body line\n");
  });

  it("reports every refusal in the batch, not only the last one", async () => {
    // The screen holds one line, so an author who dropped five files and had
    // two refused has to be able to read which two.
    const target = mount();
    listMock.mockResolvedValueOnce([]);
    uploadMock.mockImplementation((_domain, path) =>
      Promise.resolve({ path, mime: "image/png", size: 8, sha256: "a" }),
    );
    const errors: (string | null)[] = [];

    const inserted = await uploadAttachments({
      domain: "eng",
      files: [
        fileOf("tool.exe"),
        fileOf("shot.png"),
        fileOf("huge.png", 10 * 1024 * 1024 + 1),
      ],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(inserted).toBe(1);
    const reported = errors.at(-1) ?? "";
    expect(reported).toContain("tool.exe");
    expect(reported).toContain("huge.png");
    // One report per batch rather than one per failure: the clear, then this.
    expect(errors).toHaveLength(2);
  });

  it("inserts nothing for a failed upload and surfaces the server's words", async () => {
    const target = mount();
    listMock.mockResolvedValueOnce([]);
    uploadMock.mockRejectedValueOnce(
      new ApiProblem(413, "too large", "the attachment is larger than 10 MiB"),
    );
    const errors: (string | null)[] = [];

    const inserted = await uploadAttachments({
      domain: "eng",
      files: [fileOf("shot.png")],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(inserted).toBe(0);
    expect(errors.at(-1)).toBe("the attachment is larger than 10 MiB");
    expect(target.state.doc.toString()).toBe("Body line\n");
  });

  it("says so when the bytes landed but the reference could not be inserted", async () => {
    // The buffer's whole text is selected and its frontmatter is folded, which
    // is the case `insertBlock` refuses: an edit made through a block nobody
    // can see. The PUT has already happened by then, so silence here would
    // leave a stored file no engram references - the orphan the maintenance
    // sweep would raise days later.
    const doc = "---\ntitle: T\n---\n\nBody line\n";
    view = new EditorView({
      state: EditorState.create({
        doc,
        selection: EditorSelection.single(0, doc.length),
        extensions: [frontmatterFold()],
      }),
    });
    const target = view;
    listMock.mockResolvedValueOnce([]);
    uploadMock.mockImplementation((_domain, path) =>
      Promise.resolve({ path, mime: "image/png", size: 8, sha256: "a" }),
    );
    const errors: (string | null)[] = [];

    const inserted = await uploadAttachments({
      domain: "eng",
      files: [fileOf("shot.png")],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(uploadMock).toHaveBeenCalledTimes(1);
    expect(inserted).toBe(0);
    // Named, so the file can be linked by hand or deleted.
    expect(errors.at(-1)).toContain("assets/2026/08/shot.png");
    expect(target.state.doc.toString()).toBe(doc);
  });

  it("uploads nothing when the domain's attachments cannot be listed", async () => {
    const target = mount();
    listMock.mockRejectedValueOnce(
      new ApiProblem(403, "forbidden", "this domain is not readable"),
    );
    const errors: (string | null)[] = [];

    const inserted = await uploadAttachments({
      domain: "eng",
      files: [fileOf("shot.png")],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(inserted).toBe(0);
    expect(uploadMock).not.toHaveBeenCalled();
    expect(errors.at(-1)).toBe("this domain is not readable");
  });

  it("clears a standing error when a new batch starts", async () => {
    const target = mount();
    listMock.mockResolvedValueOnce([]);
    uploadMock.mockImplementation((_domain, path) =>
      Promise.resolve({ path, mime: "image/png", size: 8, sha256: "a" }),
    );
    const errors: (string | null)[] = [];

    await uploadAttachments({
      domain: "eng",
      files: [fileOf("shot.png")],
      view: target,
      onError: (message) => errors.push(message),
      now: new Date(2026, 7, 3),
    });

    expect(errors[0]).toBeNull();
  });
});
