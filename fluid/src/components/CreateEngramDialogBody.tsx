/**
 * A new engram, from wherever the thought struck: title, a folder picked off
 * the domain's own tree, type and status as suggestions that never enforce.
 * The server builds the frontmatter and slugifies the title; the answer is
 * the detail read of what landed, and the flow ends inside the editor on it.
 *
 * Split from `CreateEngramDialog.tsx` behind a lazy import: the dialog
 * primitive is otherwise not part of the entry bundle at all (every other
 * eager screen uses a dropdown or a toast, never a Radix dialog), so pulling
 * it in for a form nobody sees before clicking "New engram" would grow every
 * visit's first paint for a feature most loads never touch.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { domainTreeKey, treeQuery } from "../api/domain";
import { engramDetailKey } from "../api/engram";
import {
  fetchTags,
  fetchVocabulary,
  fullVocabularyKey,
  vocabularyKey,
} from "../api/vocabulary";
import { createEngram } from "../api/writes";
import { editRoute } from "../paths";
import {
  STATUS_SUGGESTIONS,
  TYPE_SUGGESTIONS,
  withHouseCounts,
} from "../suggestions";
import type { CreateEngramDialogProps } from "./CreateEngramDialog";
import { BUTTON, Field } from "./primitives";
import { SuggestInput, suggestionsAreOpen } from "./SuggestInput";

const FIELD_CLASSES =
  "w-full rounded border border-slate-300 bg-transparent px-2 py-1 text-sm focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700";

export default function CreateEngramDialogBody({
  domain,
  initialFolder,
  onClose,
}: CreateEngramDialogProps): ReactElement {
  const navigate = useNavigate();
  const titleField = useId();
  const tagsField = useId();
  const typeField = useId();
  const statusField = useId();
  const queryClient = useQueryClient();
  const [title, setTitle] = useState("");
  const [folder, setFolder] = useState(initialFolder);
  const [engramType, setEngramType] = useState("");
  const [status, setStatus] = useState("");
  const [tags, setTags] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  // The domain's tag vocabulary, under the same key DomainHome caches it: an
  // author who arrived from the domain page pays nothing on the wire.
  const knownTags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
  });

  // The `type` and `status` words this domain already writes, under the key
  // the editor caches the same payload at. A key of its own rather than the
  // tag one above, because the two parse the same route into different
  // shapes - see `fullVocabularyKey`. An unread or failed query is simply no
  // house words: the recommended sets stand on their own and the form works
  // exactly as it did before, so there is nothing here to wait for.
  const house = useQuery({
    queryKey: fullVocabularyKey(domain),
    queryFn: () => fetchVocabulary(domain),
  });

  const create = useMutation({
    mutationFn: () => {
      const tagList = tags
        .split(",")
        .map((tag) => tag.trim())
        .filter((tag) => tag !== "");
      return createEngram(domain, {
        title: title.trim(),
        content: "",
        ...(folder !== "" ? { folder } : {}),
        ...(engramType.trim() !== "" ? { type: engramType.trim() } : {}),
        ...(status.trim() !== "" ? { status: status.trim() } : {}),
        ...(tagList.length > 0 ? { tags: tagList } : {}),
      });
    },
    onSuccess: (created) => {
      // The editor reads through the same key: seeded here, the route it is
      // about to land on opens on what already landed rather than refetching
      // it and flashing a loading skeleton for a detail already in hand.
      queryClient.setQueryData(
        engramDetailKey(created.domain, created.permalink),
        created,
      );
      // The tree is what the sidebar and this dialog's own folder picker are
      // drawn from, and a create is a row that was not in it a moment ago.
      // Every level of the domain at once rather than the one folder: a new
      // engram can make a folder that no level had listed before.
      void queryClient.invalidateQueries({
        queryKey: domainTreeKey(created.domain),
      });
      onClose();
      void navigate(editRoute(created.domain, created.permalink));
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

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
        <Dialog.Content
          className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900"
          onEscapeKeyDown={(event) => {
            // Escape belongs to the innermost thing it can close. A suggestion
            // list is open inside this form often enough that closing the
            // whole dialog on it would throw away a half-written engram, and
            // the field cannot claim the key for itself: this layer's listener
            // runs first, on the document, in the capture phase.
            if (suggestionsAreOpen()) {
              event.preventDefault();
            }
          }}
        >
          <Dialog.Title className="text-lg font-semibold">
            New engram in {domain}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            The title becomes the filename and the address; everything else can
            change later.
          </Dialog.Description>
          <form
            className="mt-3 flex flex-col gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (title.trim() !== "" && !create.isPending) {
                setProblem(null);
                create.mutate();
              }
            }}
          >
            {problem && (
              <p
                role="alert"
                className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
              >
                {problem}
              </p>
            )}
            <Field id={titleField} label="Title">
              <input
                id={titleField}
                className={FIELD_CLASSES}
                value={title}
                onChange={(event) => {
                  setTitle(event.target.value);
                }}
                // The first field a fresh dialog wants filled.
                autoFocus
              />
            </Field>
            <FolderPicker
              domain={domain}
              chosen={folder}
              onChoose={setFolder}
            />
            <Field
              id={tagsField}
              label="Tags"
              helper="Optional, comma separated."
            >
              <input
                id={tagsField}
                aria-describedby={`${tagsField}-help`}
                className={FIELD_CLASSES}
                list="create-tags"
                value={tags}
                onChange={(event) => {
                  setTags(event.target.value);
                }}
                placeholder="rust, editing"
              />
              <datalist id="create-tags">
                {(knownTags.data ?? []).map((tag) => (
                  <option key={tag.name} value={tag.name} />
                ))}
              </datalist>
            </Field>
            {/*
              The suggesting input rather than a datalist: focus opens the
              whole recommended vocabulary with a line each on what the words
              are for, which is what a field nobody has memorized needs, and
              the words this domain already writes come with it. The helper
              stays anyway - a list of words is what a closed set looks like,
              and that anything else is allowed is the one thing the popover
              cannot say for itself.
            */}
            <Field
              id={typeField}
              label="Type"
              helper="Suggestions; any value is allowed."
            >
              <SuggestInput
                id={typeField}
                label="Type"
                describedBy={`${typeField}-help`}
                className={FIELD_CLASSES}
                value={engramType}
                suggestions={withHouseCounts(
                  TYPE_SUGGESTIONS,
                  house.data?.types ?? [],
                )}
                onChange={setEngramType}
                placeholder="engram"
              />
            </Field>
            {/* The same treatment as Type, because it is the same kind of
                field: a free-form value with a list of usual ones beside it. */}
            <Field
              id={statusField}
              label="Status"
              helper="Suggestions; any value is allowed."
            >
              <SuggestInput
                id={statusField}
                label="Status"
                describedBy={`${statusField}-help`}
                className={FIELD_CLASSES}
                value={status}
                suggestions={withHouseCounts(
                  STATUS_SUGGESTIONS,
                  house.data?.statuses ?? [],
                )}
                onChange={setStatus}
                placeholder="stable"
              />
            </Field>
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className={BUTTON.secondary}
              >
                Cancel
              </button>
              {/*
                The primary tier, which is also what makes the wait for a title
                legible: its disabled face is a filled button gone grey rather
                than an outline at half opacity, so a Create that cannot run
                yet reads as waiting rather than as broken.
              */}
              <button
                type="submit"
                disabled={title.trim() === "" || create.isPending}
                className={BUTTON.primary}
              >
                Create
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The folder picker: the domain's own tree, walked lazily exactly as the
 * sidebar walks it (same query keys, so it is usually already cached), with
 * a radio per folder. The root is always offered.
 */
function FolderPicker({
  domain,
  chosen,
  onChoose,
}: {
  domain: string;
  chosen: string;
  onChoose: (folder: string) => void;
}) {
  return (
    <fieldset className="flex flex-col gap-1 text-sm">
      <legend className="pb-1">Folder</legend>
      <label className="flex items-center gap-2">
        <input
          type="radio"
          name="create-folder"
          checked={chosen === ""}
          onChange={() => {
            onChoose("");
          }}
        />
        {/* The proportional face the other options wear: this is one choice
            in a list of folder names, not a path being quoted. */}
        <span>(root)</span>
      </label>
      <PickerBranch
        domain={domain}
        path=""
        chosen={chosen}
        onChoose={onChoose}
      />
    </fieldset>
  );
}

function PickerBranch({
  domain,
  path,
  chosen,
  onChoose,
}: {
  domain: string;
  path: string;
  chosen: string;
  onChoose: (folder: string) => void;
}) {
  const tree = useQuery(treeQuery(domain, path));
  const folders = tree.data?.folders ?? [];
  if (folders.length === 0) {
    return null;
  }
  return (
    <ul
      className={
        path === "" ? "flex flex-col gap-1" : "ml-4 flex flex-col gap-1"
      }
    >
      {folders.map((name) => {
        const full = path === "" ? name : `${path}/${name}`;
        return (
          <li key={full} className="flex flex-col gap-1">
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="create-folder"
                aria-label={name}
                checked={chosen === full}
                onChange={() => {
                  onChoose(full);
                }}
              />
              <span>{name}</span>
            </label>
            {/* Walk deeper only along the chosen path: lazy, like the sidebar. */}
            {(chosen === full || chosen.startsWith(`${full}/`)) && (
              <PickerBranch
                domain={domain}
                path={full}
                chosen={chosen}
                onChoose={onChoose}
              />
            )}
          </li>
        );
      })}
    </ul>
  );
}
