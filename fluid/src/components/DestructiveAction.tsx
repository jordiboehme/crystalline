/**
 * The two-step confirm this app uses for every destructive control: a trigger
 * that asks, a second press that means it, focus handed back to the trigger
 * on Escape or on losing the confirm without pressing it.
 *
 * Its own module because three surfaces now build on it - the members card's
 * remove, leave and transfer rows, and the domain page's danger zone - and a
 * second copy of the same machinery is exactly what this pattern exists to
 * prevent. The precedent it repeats is `Profile.tsx`'s `TokenRow`: a dialog
 * the browser owns cannot be reached by a test, styled, or dismissed by
 * keyboard the way this can.
 *
 * Three things a caller can ask for beyond a plain confirm:
 *
 * * `children`, an extra control rendered between trigger and confirm that
 *   carries its own value through to `onConfirm` - the members card's
 *   "Transfer ownership" types a login name into one;
 * * `requireMatch`, a name that has to be typed exactly before the confirm
 *   will fire, for the losses no second press alone is proportionate to -
 *   unregistering a domain, or changing who may see it;
 * * `confirming`/`onConfirmingChange`, for a parent that arms the
 *   confirmation from somewhere else entirely, which is what the domain
 *   page's command palette row does. Left out, the control owns the flag
 *   itself and nothing about a caller that never passes it changes.
 */

import type { ReactElement, ReactNode } from "react";
import { useEffect, useId, useRef, useState } from "react";

import { BUTTON, FIELD } from "./primitives";

/**
 * `readOnly` is a certainty this side already holds, straight off the
 * capability probe - unlike a per-domain right, nothing is gained by
 * offering a control the client can prove is refused. A control carrying
 * this is disabled rather than removed, though, and the sentence rides along
 * as its accessible description: a door that will not open is still the
 * door, shown as one, not vanished the way a merely *derived* right's
 * absence is.
 *
 * Here rather than in one of the cards, because both of them say it and a
 * sentence the reader meets twice must not be able to drift into two.
 */
export const READ_ONLY_REASON =
  "This instance is read only, so nothing here can be changed.";

export function DestructiveAction({
  label,
  confirmLabel,
  ariaLabel,
  confirmAriaLabel,
  pending,
  disabledReason,
  requireValue = false,
  requireMatch,
  confirming,
  onConfirmingChange,
  children,
  onConfirm,
}: {
  label: string;
  confirmLabel: string;
  /**
   * The trigger's accessible name, when the visible `label` alone would not
   * be unique on the page - a row's "Remove" beside every other row's own.
   * Defaults to `label`.
   */
  ariaLabel?: string;
  /** Same reason, for the confirm press. Defaults to `confirmLabel`. */
  confirmAriaLabel?: string;
  pending: boolean;
  /**
   * When set, the trigger is disabled and this is exposed as its accessible
   * description - a certainty already held, not a right this side is merely
   * guessing at, so the door is shown shut rather than removed.
   */
  disabledReason?: string | undefined;
  /** Whether the confirm press needs a non-empty `value` before it may fire. */
  requireValue?: boolean;
  /**
   * The name that has to be typed before the confirm press will fire, shown
   * in code face in the field's own label. Compared exactly: no trimming and
   * no case folding, because the point of typing it is to have read it.
   */
  requireMatch?: string;
  /** The confirmation's state, for a parent that arms it from elsewhere. */
  confirming?: boolean;
  /** Told whenever the confirmation opens or closes, controlled or not. */
  onConfirmingChange?: (confirming: boolean) => void;
  /** An extra control rendered between trigger and confirm, e.g. a text field. */
  children?: (value: string, setValue: (value: string) => void) => ReactNode;
  onConfirm: (value: string) => void;
}): ReactElement {
  const [ownConfirming, setOwnConfirming] = useState(false);
  const [value, setValue] = useState("");
  const trigger = useRef<HTMLButtonElement>(null);
  const name = ariaLabel ?? label;
  const confirmName = confirmAriaLabel ?? confirmLabel;
  const reasonId = useId();
  const fieldId = useId();
  const open = confirming ?? ownConfirming;

  function setOpen(next: boolean) {
    // The parent hears about it either way; only an uncontrolled caller's
    // flag lives here, so a controlled one cannot end up with two answers.
    if (confirming === undefined) {
      setOwnConfirming(next);
    }
    onConfirmingChange?.(next);
  }

  // `aria-disabled`, not `disabled`, on both buttons below: `disabled` takes
  // a control out of the tab order entirely, so a keyboard user could never
  // land on it, let alone hear the reason `aria-describedby` attaches to it.
  // This repo already ruled on exactly this trade in `Layout.tsx`'s own
  // `ShareChanges` - reachable by pointer and by keyboard, says it will not
  // act, and does not, because the press is guarded as well as the face.
  const disabled = pending || disabledReason !== undefined;
  const confirmDisabled =
    pending ||
    disabledReason !== undefined ||
    (requireValue && value.trim() === "") ||
    (requireMatch !== undefined && value !== requireMatch);

  function abandon() {
    setOpen(false);
    setValue("");
    trigger.current?.focus();
  }

  // The safety net for every path that collapses the confirmation without
  // going through `abandon()` - a controlled parent whose mutation was
  // refused is the one that matters, since its `onError` sets the flag false
  // and has no way to reach this ref. Escape and "Keep" both already call
  // `abandon()` and focus the trigger synchronously, so by the time this
  // effect runs afterward focus is already there and the check below is a
  // no-op for them.
  //
  // The body check is the discriminator that keeps the deliberate blur path
  // honest: that path also collapses the confirmation, but BECAUSE focus
  // already moved somewhere else on purpose - stealing it back here would
  // undo that intent. When the confirm buttons unmount out from under a
  // refusal, the browser drops focus to the document body, which is exactly
  // what distinguishes "focus was lost" from "focus moved on purpose".
  //
  // The typed value goes with the closing, whichever path closed it: a
  // confirmation that re-opens pre-filled would be one press from the loss
  // it exists to slow down.
  const wasOpen = useRef(open);
  useEffect(() => {
    if (wasOpen.current && !open) {
      setValue("");
      const active = document.activeElement;
      if (active === document.body || active === null) {
        trigger.current?.focus();
      }
    }
    wasOpen.current = open;
  }, [open]);

  return (
    <span
      className="inline-flex flex-wrap items-center gap-2"
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.stopPropagation();
          abandon();
        }
      }}
      onBlur={(event) => {
        // Only when the focus actually landed somewhere the reader chose. A
        // `focusout` with no destination is what a click looks like
        // mid-flight, and taking the confirmation away there would eat the
        // second press this exists to require. Nor is a destination that is
        // out of the tab order a choice: a dialog closing behind this one
        // parks focus on a container of its own on the way out - which is
        // exactly what the command palette does to the row that armed this -
        // and that is the dialog leaving rather than the reader.
        const next = event.relatedTarget;
        if (
          open &&
          next instanceof HTMLElement &&
          next.tabIndex >= 0 &&
          !event.currentTarget.contains(next)
        ) {
          setOpen(false);
        }
      }}
    >
      <button
        ref={trigger}
        type="button"
        aria-label={name}
        aria-expanded={open}
        aria-describedby={disabledReason !== undefined ? reasonId : undefined}
        aria-disabled={disabled}
        onClick={() => {
          if (disabled) {
            return;
          }
          setOpen(true);
        }}
        className={`${BUTTON.destructive} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
      >
        {label}
      </button>
      {disabledReason !== undefined && (
        <span id={reasonId} className="sr-only">
          {disabledReason}
        </span>
      )}
      {open && (
        <>
          {requireMatch === undefined ? (
            children?.(value, setValue)
          ) : (
            <>
              <label
                htmlFor={fieldId}
                className="text-sm text-slate-600 dark:text-slate-400"
              >
                Type{" "}
                <code className="rounded bg-slate-100 px-1 font-mono dark:bg-slate-800">
                  {requireMatch}
                </code>{" "}
                to confirm
              </label>
              <input
                id={fieldId}
                autoFocus
                required
                autoComplete="off"
                value={value}
                onChange={(event) => {
                  setValue(event.target.value);
                }}
                className={`w-40 ${FIELD}`}
              />
            </>
          )}
          <button
            type="button"
            // The field, where there is one, is what the confirmation opens
            // on: a press that cannot fire yet is no place to land.
            autoFocus={children === undefined && requireMatch === undefined}
            aria-label={confirmName}
            aria-describedby={
              disabledReason !== undefined ? reasonId : undefined
            }
            aria-disabled={confirmDisabled}
            onClick={() => {
              if (confirmDisabled) {
                return;
              }
              setOpen(false);
              const confirmed = value;
              setValue("");
              onConfirm(confirmed);
            }}
            className={`${BUTTON.destructive} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
          >
            {confirmLabel}
          </button>
          <button type="button" onClick={abandon} className={BUTTON.secondary}>
            Keep
          </button>
        </>
      )}
    </span>
  );
}
