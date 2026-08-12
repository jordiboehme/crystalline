/**
 * Importing an archive: a dry run first, then the write it described.
 *
 * The two calls are the whole design. An import is the one write in this app
 * that can land hundreds of engrams at once, and the two things it can do that
 * nobody wants - overwrite an engram somebody is still working on, or refuse
 * half an archive for a reason discovered too late - are both visible in
 * advance, because the server offers a preview that reads the same archive
 * through the same code and writes nothing. So this dialog will not let the
 * import run until that report is on screen: `Import` is disabled until a
 * preview has answered, and picking a different file takes the report away
 * again rather than letting it stand for bytes it was never about.
 *
 * The report is shown entry by entry rather than as counters, because the
 * counters cannot answer the question an admin actually has, which is WHICH
 * engram collides and WHY an entry will not be written. The verify findings
 * ride along for exactly that reason.
 *
 * Split from `ImportArchiveDialog.tsx` behind a lazy import, for the reason
 * every dialog body here is: the Radix dialog, this table and the archive
 * verbs are otherwise in the entry bundle, paid for by every session that
 * never imports anything - which is nearly all of them.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useRef, useState } from "react";

import { importArchive, previewArchive } from "../api/admin";
import type { ArchiveEntry, ArchiveReport } from "../api/admin";
import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import type { ImportArchiveDialogProps } from "./ImportArchiveDialog";
import { BUTTON, Chip, FIELD } from "./primitives";
import type { ChipVariant } from "./primitives";

/** What happens to an entry whose path or permalink is already taken. */
type Policy = "skip" | "overwrite";

const POLICIES: { policy: Policy; label: string; helper: string }[] = [
  {
    policy: "skip",
    label: "Skip existing",
    helper: "An engram already at that address is left exactly as it is.",
  },
  {
    policy: "overwrite",
    label: "Overwrite existing",
    helper: "The archive's version replaces what is there.",
  },
];

/**
 * Which chip a report status wears.
 *
 * Guidance-shaped, like `statusVariant` next door: the two vocabularies the
 * server uses - a preview's and an import's - are mapped by what they mean for
 * the engram at the other end, and anything else stays neutral rather than
 * being announced as a problem. `invalid` is the one that gets red: it is the
 * only status that means the entry cannot be written at all, while `collides`
 * and `skipped` mean something is already there, which is a choice rather than
 * a fault.
 */
function statusChip(status: string): ChipVariant {
  switch (status) {
    case "new":
    case "created":
    case "overwritten":
      return "positive";
    case "collides":
    case "skipped":
      return "caution";
    case "invalid":
      return "danger";
    default:
      return "neutral";
  }
}

export default function ImportArchiveDialogBody({
  domain,
  onClose,
}: ImportArchiveDialogProps): ReactElement {
  const queryClient = useQueryClient();
  const fileField = useId();
  const policyGroup = useId();
  const [file, setFile] = useState<File | null>(null);
  const [policy, setPolicy] = useState<Policy>("skip");
  const [preview, setPreview] = useState<ArchiveReport | null>(null);
  const [result, setResult] = useState<ArchiveReport | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  /**
   * Which archive the reports on screen are allowed to be about.
   *
   * A real archive is megabytes over the wire, so a dry run is seconds long,
   * and "wrong file, let me pick the other one" is the most ordinary gesture
   * there is. Without this, the superseded run for the first file lands over
   * the cleared state, draws ITS entries, arms `Import` - and the import then
   * ships the second file's bytes, which nothing ever dry-ran. A ref rather
   * than the state above because a callback closes over the state it was
   * created with, and this has to be read as of the moment the answer arrives.
   */
  const picked = useRef<File | null>(null);

  // Derived rather than stored, so the three states cannot disagree with the
  // two reports they are made of: nothing picked yet, a dry run in hand, or an
  // import that has landed.
  const step: "pick" | "previewed" | "done" =
    result !== null ? "done" : preview !== null ? "previewed" : "pick";

  const dryRun = useMutation({
    mutationFn: async (chosen: File) =>
      previewArchive(domain, await chosen.arrayBuffer()),
    // Both callbacks take the file the call was made with and drop the answer
    // when it is no longer the file on the form: a report is about the bytes
    // it was made from, and a report about bytes nobody is offering any more
    // is not news, it is a lie about what would happen next.
    onSuccess: (report, chosen) => {
      if (chosen === picked.current) {
        setPreview(report);
      }
    },
    onError: (error: Error, chosen) => {
      if (chosen !== picked.current) {
        return;
      }
      // A refusal of the archive itself - a path escaping the domain root, a
      // file that is not a zip - is about the whole upload, so no half-report
      // stands beside it.
      setPreview(null);
      setProblem(problemDetail(error));
    },
  });

  const write = useMutation({
    mutationFn: async (chosen: File) =>
      importArchive(domain, await chosen.arrayBuffer(), policy),
    onSuccess: (report) => {
      setResult(report);
      // An import moves the shape of the domain and the number of engrams in
      // it: every folder of the tree this domain draws its navigation from,
      // and the listing every sidebar, card and switcher reads.
      void queryClient.invalidateQueries({ queryKey: domainTreeKey(domain) });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const busy = dryRun.isPending || write.isPending;
  // What is on screen: the import's own report once there is one, so the
  // statuses a reader ends up looking at are what happened rather than what
  // would have happened.
  const report = result ?? preview;

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 max-h-[calc(100vh-4rem)] w-[min(44rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Import archive
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {`A zip of markdown files, written into ${domain}. Nothing is written until a preview has said what would happen.`}
          </Dialog.Description>

          <div className="mt-3 flex flex-col gap-3">
            {problem !== null && (
              <p
                role="alert"
                className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
              >
                {problem}
              </p>
            )}

            {step !== "done" && (
              <div className="flex flex-col gap-1 text-sm">
                <label htmlFor={fileField}>Archive file</label>
                <input
                  id={fileField}
                  type="file"
                  accept=".zip"
                  className={`w-full py-1 ${FIELD}`}
                  onChange={(event) => {
                    // A report is about the bytes it was made from. Picking
                    // another archive takes it away rather than leaving an
                    // Import button armed over the wrong file - including the
                    // report of a dry run that is still out, which the two
                    // callbacks above drop by comparing against this.
                    const chosen = event.target.files?.[0] ?? null;
                    picked.current = chosen;
                    setFile(chosen);
                    setPreview(null);
                    setProblem(null);
                  }}
                />
              </div>
            )}

            {step !== "done" && (
              <fieldset className="flex flex-col gap-2 text-sm">
                <legend className="pb-1">Entries already in the domain</legend>
                {POLICIES.map((option) => (
                  <div key={option.policy} className="flex flex-col">
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        name={policyGroup}
                        value={option.policy}
                        checked={policy === option.policy}
                        aria-describedby={`${policyGroup}-${option.policy}`}
                        onChange={() => {
                          setPolicy(option.policy);
                        }}
                      />
                      <span>{option.label}</span>
                    </label>
                    <p
                      id={`${policyGroup}-${option.policy}`}
                      className="text-caption pl-6 text-slate-500 dark:text-slate-400"
                    >
                      {option.helper}
                    </p>
                  </div>
                ))}
              </fieldset>
            )}

            {report !== null && (
              <EntryTable entries={report.entries} imported={result !== null} />
            )}

            {result !== null && (
              <p className="text-sm tabular-nums">
                {`${result.written} written, ${result.skipped} skipped, ${result.invalid} invalid, ${result.ignored} ignored.`}
              </p>
            )}

            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className={BUTTON.secondary}
              >
                {step === "done" ? "Close" : "Cancel"}
              </button>
              {step !== "done" && (
                <>
                  <button
                    type="button"
                    disabled={file === null || busy}
                    onClick={() => {
                      if (file !== null) {
                        setProblem(null);
                        dryRun.mutate(file);
                      }
                    }}
                    className={BUTTON.secondary}
                  >
                    Preview
                  </button>
                  {/*
                    The primary tier, and the only control here that writes:
                    disabled until a dry run has answered, so the button and
                    the report on screen are always about the same archive.
                  */}
                  <button
                    type="button"
                    disabled={file === null || preview === null || busy}
                    onClick={() => {
                      if (file !== null) {
                        setProblem(null);
                        write.mutate(file);
                      }
                    }}
                    className={BUTTON.primary}
                  >
                    Import
                  </button>
                </>
              )}
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * Every entry of the archive and what became - or would become - of it.
 *
 * The paths are the archive's own and can be long, so the table scrolls
 * sideways inside its own box rather than pushing the dialog wider than the
 * window.
 */
function EntryTable({
  entries,
  imported,
}: {
  entries: ArchiveEntry[];
  /** Whether this is what happened, rather than what would happen. */
  imported: boolean;
}): ReactElement {
  return (
    <div className="overflow-x-auto">
      <table
        aria-label={
          imported ? "What the import did" : "What an import would do"
        }
        className="w-full text-left text-sm"
      >
        <thead className="text-caption text-slate-500 dark:text-slate-400">
          <tr>
            <th scope="col" className="py-1 pr-3 font-normal">
              Path
            </th>
            <th scope="col" className="py-1 pr-3 font-normal">
              Status
            </th>
            <th scope="col" className="py-1 font-normal">
              Detail
            </th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => (
            <tr
              key={entry.path}
              className="border-t border-slate-200 align-top dark:border-slate-800"
            >
              <td className="py-1 pr-3 font-mono break-all">{entry.path}</td>
              <td className="py-1 pr-3">
                <Chip variant={statusChip(entry.status)}>{entry.status}</Chip>
              </td>
              <td className="py-1 text-slate-600 dark:text-slate-400">
                {entry.reason !== null && <span>{entry.reason}</span>}
                {entry.findings.length > 0 && (
                  <ul className="flex flex-col gap-0.5">
                    {entry.findings.map((finding, index) => (
                      <li key={`${finding.rule}-${String(index)}`}>
                        {finding.line !== null && (
                          <span className="tabular-nums">{`Line ${String(finding.line)}: `}</span>
                        )}
                        {finding.message}
                      </li>
                    ))}
                  </ul>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
