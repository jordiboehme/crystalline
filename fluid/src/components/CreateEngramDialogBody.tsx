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
import { useState } from "react";
import { useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import { fetchTree, treeKey } from "../api/domain";
import { engramDetailKey } from "../api/engram";
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import { createEngram } from "../api/writes";
import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "../filters";
import { editRoute } from "../paths";
import type { CreateEngramDialogProps } from "./CreateEngramDialog";

const FIELD_CLASSES =
  "w-full rounded border border-slate-300 bg-transparent px-2 py-1 text-sm focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700";

export default function CreateEngramDialogBody({
  domain,
  initialFolder,
  onClose,
}: CreateEngramDialogProps): ReactElement {
  const navigate = useNavigate();
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
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
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
            <label className="flex flex-col gap-1 text-sm">
              <span>Title</span>
              <input
                className={FIELD_CLASSES}
                value={title}
                onChange={(event) => {
                  setTitle(event.target.value);
                }}
                // The first field a fresh dialog wants filled.
                autoFocus
              />
            </label>
            <FolderPicker
              domain={domain}
              chosen={folder}
              onChoose={setFolder}
            />
            <label className="flex flex-col gap-1 text-sm">
              <span>Tags (optional, comma separated)</span>
              <input
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
            </label>
            <label className="flex flex-col gap-1 text-sm">
              <span>Type (suggestions, free form)</span>
              <input
                className={FIELD_CLASSES}
                list="create-types"
                value={engramType}
                onChange={(event) => {
                  setEngramType(event.target.value);
                }}
                placeholder="engram"
              />
              <datalist id="create-types">
                {SUGGESTED_TYPES.map((name) => (
                  <option key={name} value={name} />
                ))}
              </datalist>
            </label>
            <label className="flex flex-col gap-1 text-sm">
              <span>Status (suggestions, free form)</span>
              <input
                className={FIELD_CLASSES}
                list="create-statuses"
                value={status}
                onChange={(event) => {
                  setStatus(event.target.value);
                }}
                placeholder="stable"
              />
              <datalist id="create-statuses">
                {SUGGESTED_STATUSES.map((name) => (
                  <option key={name} value={name} />
                ))}
              </datalist>
            </label>
            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800"
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={title.trim() === "" || create.isPending}
                className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800"
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
        <span className="font-mono text-xs">(root)</span>
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
  const tree = useQuery({
    queryKey: treeKey(domain, path),
    queryFn: () => fetchTree(domain, path),
  });
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
