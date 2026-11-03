/**
 * The "Link" popover: the address of a link, or a new link's address (and
 * its text, at a caret). Save or Enter writes only that: the address span of
 * an existing link (its title stays), or the brackets around a selection.
 * "Open link" opens a web or mail address in a new tab, never in the editor;
 * "Remove link" takes the two link marks away and keeps the words.
 *
 * A reference link's address is its definition's, shared with every other
 * link that names it, so it is shown read only here.
 */

import type { ReactElement } from "react";
import { useId, useState } from "react";

import { BUTTON, FIELD } from "../../components/primitives";
import {
  linkAddress,
  linkChanges,
  newLinkChanges,
  openableAddress,
  unlinkChanges,
  writtenAddress,
} from "../elementVerbs";
import type { PopoverBodyProps } from "./ElementPopover";

export function LinkPopover({
  view,
  edit,
  commit,
  live,
}: PopoverBodyProps): ReactElement | null {
  const id = useId();
  const existing = edit.kind === "link";
  const [initial] = useState(() =>
    edit.kind === "link" ? (linkAddress(view.state, edit.at) ?? "") : "",
  );
  const [address, setAddress] = useState(initial);
  const [text, setText] = useState("");
  if (edit.kind !== "link" && edit.kind !== "new-link") return null;
  const reference = edit.kind === "link" && edit.reference;
  const atCaret = edit.kind === "new-link" && edit.range.from === edit.range.to;
  const valid = writtenAddress(address) !== null;
  const openable = existing ? openableAddress(initial) : null;

  const save = () => {
    if (reference) {
      // Nothing here to write: the focus goes back.
      commit(() => []);
      return;
    }
    if (!valid) return;
    // Only what the person changed is written: an untouched address keeps
    // whatever the document holds now (a peer may have changed it).
    if (existing && address === initial) {
      commit(() => []);
      return;
    }
    commit((state, now) => {
      if (now.kind === "link") return linkChanges(state, now.at, { address });
      if (now.kind === "new-link") {
        return newLinkChanges(state, now.range, { text, address });
      }
      return null;
    });
  };
  /**
   * Points "Open link" at the address the document holds now, which a peer
   * may have changed since the popover opened. False when there is none to
   * open any more.
   */
  const followLive = (event: { currentTarget: HTMLAnchorElement }): boolean => {
    const now = live();
    const href =
      now?.kind === "link"
        ? openableAddress(linkAddress(view.state, now.at) ?? "")
        : null;
    if (href === null) return false;
    event.currentTarget.setAttribute("href", href);
    return true;
  };
  const remove = () => {
    commit((state, now) =>
      now.kind === "link" ? unlinkChanges(state, now.at) : null,
    );
  };

  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        save();
      }}
    >
      {atCaret && (
        <div className="flex flex-col gap-1">
          <label htmlFor={`${id}-text`}>Link text</label>
          <input
            id={`${id}-text`}
            className={FIELD}
            value={text}
            onChange={(event) => {
              setText(event.target.value);
            }}
          />
        </div>
      )}
      <div className="flex flex-col gap-1">
        <label htmlFor={`${id}-address`}>Address</label>
        <input
          id={`${id}-address`}
          className={FIELD}
          value={address}
          readOnly={reference}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => {
            setAddress(event.target.value);
          }}
        />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {!reference && (
          <button type="submit" className={BUTTON.primary} disabled={!valid}>
            Save
          </button>
        )}
        {existing &&
          (openable !== null ? (
            <a
              href={openable}
              target="_blank"
              rel="noopener noreferrer"
              className={BUTTON.secondary}
              onFocus={followLive}
              onPointerEnter={followLive}
              onClick={(event) => {
                if (!followLive(event)) event.preventDefault();
              }}
            >
              Open link
            </a>
          ) : (
            <button type="button" className={BUTTON.secondary} disabled>
              Open link
            </button>
          ))}
        {existing && (
          <button type="button" className={BUTTON.ghost} onClick={remove}>
            Remove link
          </button>
        )}
      </div>
    </form>
  );
}
