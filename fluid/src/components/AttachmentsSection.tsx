/**
 * What this engram carries with it: the files its own prose references.
 *
 * Two questions make one list. The body says which attachments this engram
 * points at, and the domain's listing says which of those actually exist -
 * neither alone is the answer. The domain's whole listing belongs to every
 * engram in it, so showing it here would put somebody else's deck on this
 * page; the body's references alone would claim a file exists because a human
 * typed its name.
 *
 * So a reference the domain does not hold says "missing" rather than
 * disappearing. That is a fact about the knowledge base - the maintenance
 * sweep raises it as a dangling reference, and this panel is where a reader
 * sees the same thing while looking at the engram that made the claim.
 *
 * Removing a file says out loud what it does NOT do. Deleting the bytes leaves
 * every reference in every engram exactly as written, because an app that
 * edited somebody's prose on a delete would be rewriting knowledge to keep its
 * own bookkeeping tidy. The confirm names that consequence and names who will
 * point at it afterwards.
 *
 * A home that can do more (the Reflection's, for a curator) also offers
 * Replace, which puts new bytes under the same path through the home's own
 * `replace` verb, and Move, which the owner opens because a move rewrites
 * other pages and its dialog outlives this list. An engram rail passes
 * neither, so it shows neither.
 */

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderInput, Replace, Trash2 } from "lucide-react";
import type { ChangeEvent, ReactElement, RefObject } from "react";
import { useEffect, useMemo, useRef, useState } from "react";

import { problemDetail } from "../api/client";
import type { AssetHome, AttachmentRow } from "../api/files";
import { assetRefsIn } from "../editor/imageFormat";
import { formatBytes } from "../format";
import { BUTTON, Chip, FOCUS_RING, IconButton, RAIL_LABEL } from "./primitives";
import { useEngramAssetHome } from "./useEngramAssetHome";

export interface AttachmentsSectionProps {
  /** The domain the paths are relative to. */
  domain: string;
  /** The engram's body: its references are what this section lists. */
  body: string;
  /** Whether this session may remove a file. A reader is offered no control. */
  canDelete?: boolean;
  /**
   * Where the files live and how the body names them. Absent, it is the
   * engram home of {@link domain}.
   */
  home?: AssetHome;
  /** The question a remove asks, by file name. Absent, the engram one. */
  confirmText?: (name: string) => string;
  /**
   * Whether this session may put new bytes under a file's own path. Needs a
   * home with a `replace` verb as well; an engram home has none.
   */
  canReplace?: boolean;
  /** Opens the owner's move dialog for a stored path. Absent, no Move. */
  onMove?: ((path: string) => void) | undefined;
  /** Called after a file is removed, for whatever else the owner refreshes. */
  onRemoved?: (path: string) => void;
}

/** The question this section has open, about one file by path. */
type Asking =
  | { kind: "remove"; path: string }
  | { kind: "replace"; path: string; file: File };

/**
 * Where focus goes once a question closes: back to the row's button that
 * asked (a no), or to the file's link (a done; the heading if it is gone).
 */
type FocusAfter =
  { kind: "opener"; key: string } | { kind: "file"; path: string };

/** Which row button opened a question: its kind and the file's path. */
function openerKey(kind: Asking["kind"], path: string): string {
  return `${kind}\u0000${path}`;
}

/**
 * Keeps a mounted element in `map` under `key` (and drops it on unmount), so
 * focus can find a row's control without asking the DOM by name. Called from
 * a callback ref, never while rendering.
 */
function remember<T extends HTMLElement>(
  map: RefObject<Map<string, T>>,
  key: string,
  element: T | null,
): void {
  if (element === null) {
    map.current.delete(key);
  } else {
    map.current.set(key, element);
  }
}

/** What removing an engram's file leaves behind, said before it happens. */
function engramConfirmText(name: string): string {
  return `Remove ${name}? Engrams keep their references until edited; evolve will flag them.`;
}

/** The file's own name, which is what a reader knows it by. */
function nameOf(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/** The extension, lowercase, or "" - the shortest honest word for the kind. */
function kindOf(path: string): string {
  const name = nameOf(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function AttachmentsSection({
  domain,
  body,
  canDelete = false,
  home: givenHome,
  confirmText = engramConfirmText,
  canReplace = false,
  onMove,
  onRemoved,
}: AttachmentsSectionProps) {
  // The domain's own files carry their current bytes in their addresses
  // (`useEngramAssetHome`); a given home brings its own.
  const engramHome = useEngramAssetHome(
    givenHome === undefined ? domain : undefined,
    body,
  );
  const home = (givenHome ?? engramHome) as AssetHome;
  const referenced = useMemo(
    () => assetRefsIn(body, home.resolve),
    [body, home],
  );
  const queryClient = useQueryClient();
  const attachments = useQuery({
    queryKey: home.key,
    queryFn: () => home.list(),
    // An engram that references no file asks the server nothing: the listing
    // would answer a question this panel is not going to draw.
    enabled: referenced.length > 0,
  });
  const [asking, setAsking] = useState<Asking | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  /** The row buttons that open a question, by {@link openerKey}. */
  const openers = useRef(new Map<string, HTMLButtonElement>());
  /** The files' name links, by path. */
  const links = useRef(new Map<string, HTMLAnchorElement>());
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const answerRef = useRef<HTMLButtonElement | null>(null);
  /** Set with the close of a question, read by the render that closes it. */
  const focusAfter = useRef<FocusAfter | null>(null);
  // A question that opens takes focus to its answer, also when it opens as
  // the file picker closes. One that closes hands it on, after the render
  // that brings the row's controls (and the refreshed listing) back.
  useEffect(() => {
    if (asking !== null) {
      answerRef.current?.focus();
      return;
    }
    const after = focusAfter.current;
    focusAfter.current = null;
    if (after === null) {
      return;
    }
    const target =
      after.kind === "opener"
        ? openers.current.get(after.key)
        : links.current.get(after.path);
    (target ?? headingRef.current)?.focus();
  }, [asking]);

  if (referenced.length === 0) {
    return null;
  }

  const held = new Map<string, AttachmentRow>(
    (attachments.data ?? []).map((row) => [row.path, row]),
  );

  const replace = canReplace ? home.replace : undefined;

  /** Runs the answered question; a failure is the server's sentence. */
  const settle = (question: Asking) => {
    void (async () => {
      setBusy(true);
      setFailure(null);
      try {
        if (question.kind === "remove") {
          await home.remove(question.path);
          onRemoved?.(question.path);
        } else {
          await replace?.(question.path, question.file);
        }
        // The listing this panel drew is stale the moment the file is gone
        // or its size changed, and it is the same listing the upload flow
        // picks a free path against, so it is refreshed rather than patched
        // in place.
        await queryClient.invalidateQueries({ queryKey: home.key });
        focusAfter.current = { kind: "file", path: question.path };
        setAsking(null);
      } catch (cause) {
        setFailure(
          cause instanceof Error ? problemDetail(cause) : String(cause),
        );
      } finally {
        setBusy(false);
      }
    })();
  };

  /** A picked file replaces only a file of its own kind. */
  const picked = (path: string, file: File) => {
    const name = nameOf(path);
    const kind = kindOf(path);
    if (kindOf(file.name) !== kind) {
      setFailure(`Pick a .${kind} file to replace ${name}.`);
      return;
    }
    setFailure(null);
    setAsking({ kind: "replace", path, file });
  };

  return (
    <section aria-label="Attachments" className="text-sm">
      <h2
        ref={headingRef}
        tabIndex={-1}
        className={`mb-2 rounded ${RAIL_LABEL} ${FOCUS_RING}`}
      >
        Attachments
      </h2>
      {failure !== null && (
        <p
          role="alert"
          className="mb-2 text-caption text-red-700 dark:text-red-300"
        >
          {failure}
        </p>
      )}
      <ul className="flex flex-col">
        {referenced.map((path) => {
          const row = held.get(path);
          const name = nameOf(path);
          const kind = kindOf(path);
          // Missing is a claim, so it waits for an answer: until the listing
          // lands, a file is neither shown as present nor accused of being
          // gone.
          const missing = row === undefined && attachments.isSuccess;
          return (
            <li key={path} className="flex flex-col gap-0.5 py-1.5">
              <span className="flex min-w-0 items-center gap-2">
                {row === undefined ? (
                  <span
                    className="min-w-0 truncate text-slate-500 dark:text-slate-400"
                    title={path}
                  >
                    {name}
                  </span>
                ) : (
                  <a
                    ref={(element) => {
                      remember(links, path, element);
                    }}
                    href={home.url(path)}
                    target="_blank"
                    rel="noreferrer"
                    title={path}
                    className={`min-w-0 truncate rounded text-link underline decoration-(--link-line) decoration-1 underline-offset-[3px] hover:no-underline ${FOCUS_RING}`}
                  >
                    {name}
                  </a>
                )}
                <span className="ml-auto flex shrink-0 items-baseline gap-2 text-caption text-muted">
                  {kind !== "" && <span className="font-mono">{kind}</span>}
                  {row !== undefined && (
                    <span className="tabular-nums">
                      {formatBytes(row.size)}
                    </span>
                  )}
                </span>
                {row !== undefined && asking === null && (
                  // Last in the row and named after the file they act on: a
                  // rail may hold several rows, and "Remove" alone would be
                  // the same name three times over.
                  <span className="flex shrink-0 items-center">
                    {replace !== undefined && kind !== "" && (
                      <ReplacePicker
                        buttonRef={(element) => {
                          remember(
                            openers,
                            openerKey("replace", path),
                            element,
                          );
                        }}
                        name={name}
                        kind={kind}
                        onPick={(file) => {
                          picked(path, file);
                        }}
                      />
                    )}
                    {onMove !== undefined && (
                      <IconButton
                        label={`Move ${name}`}
                        icon={FolderInput}
                        onClick={() => {
                          setFailure(null);
                          onMove(path);
                        }}
                      />
                    )}
                    {canDelete && (
                      <IconButton
                        ref={(element) => {
                          remember(openers, openerKey("remove", path), element);
                        }}
                        label={`Remove ${name}`}
                        icon={Trash2}
                        onClick={() => {
                          setFailure(null);
                          setAsking({ kind: "remove", path });
                        }}
                      />
                    )}
                  </span>
                )}
              </span>
              {missing && (
                <span className="flex items-center gap-2">
                  <Chip variant="caution">missing</Chip>
                </span>
              )}
              {asking?.path === path && (
                <span className="flex flex-col gap-2">
                  <span className="text-caption text-slate-600 dark:text-slate-300">
                    {asking.kind === "remove"
                      ? confirmText(name)
                      : `Replace ${name}? Every page that shows it shows the new file.`}
                  </span>
                  <span className="flex flex-wrap gap-2">
                    <button
                      type="button"
                      ref={answerRef}
                      className={
                        asking.kind === "remove"
                          ? BUTTON.destructive
                          : BUTTON.primary
                      }
                      disabled={busy}
                      onClick={() => {
                        settle(asking);
                      }}
                    >
                      {asking.kind === "remove" ? "Remove" : "Replace"}
                    </button>
                    <button
                      type="button"
                      className={BUTTON.ghost}
                      disabled={busy}
                      onClick={() => {
                        focusAfter.current = {
                          kind: "opener",
                          key: openerKey(asking.kind, asking.path),
                        };
                        setAsking(null);
                      }}
                    >
                      Cancel
                    </button>
                  </span>
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * "Replace <name>": a hidden picker limited to the file's own extension. The
 * filter is a hint every dialog offers a way past, so the section checks the
 * picked name again before it asks anything.
 */
function ReplacePicker({
  buttonRef,
  name,
  kind,
  onPick,
}: {
  /** The section keeps the button to hand focus back to it. */
  buttonRef: (element: HTMLButtonElement | null) => void;
  name: string;
  kind: string;
  onPick: (file: File) => void;
}): ReactElement {
  const picker = useRef<HTMLInputElement | null>(null);
  return (
    <>
      <IconButton
        ref={buttonRef}
        label={`Replace ${name}`}
        icon={Replace}
        onClick={() => {
          picker.current?.click();
        }}
      />
      <input
        ref={picker}
        type="file"
        accept={`.${kind}`}
        aria-label={`Replace ${name}: pick a file`}
        className="hidden"
        onChange={(event: ChangeEvent<HTMLInputElement>) => {
          const file = event.target.files?.[0];
          // Cleared first, so picking the same file again still fires.
          event.target.value = "";
          if (file !== undefined) {
            onPick(file);
          }
        }}
      />
    </>
  );
}
