/**
 * What the floating bar holds on a selected picture: where it stands (Left
 * with text wrap, Centered, Right with text wrap), how wide it is (Small,
 * Medium, Full width, Original size), its alt text in a field of its own,
 * and Remove picture. The size buttons are the keyboard's way to what the
 * resize handles do with the pointer.
 *
 * Only an attachment's size and place can be set: for any other picture the
 * two groups give way to one line that says so, and the alt text and the
 * removal stay.
 *
 * Every command runs a picture verb on the main view, one position inside
 * the picture (P22: at a junction of two pictures its start is also the end
 * of the left one), and gives the focus back to the text with the picture
 * still selected. A verb answers false for a size the picture already has
 * as much as for a refusal, so the answer is not read: a refused change
 * says why itself.
 *
 * The alt field writes nothing while it is typed in. Enter writes it and
 * returns the focus, Escape puts it back as it was and returns the focus,
 * and leaving it writes it. When the bar closes for any other reason (the
 * picture deselected, Raw, a rebuilt state), a changed field is written
 * once, if its picture is still there: the field follows the picture through
 * every change (a peer typing before it), and a picture deleted under it
 * takes the change with it. Part of the bar's chunk.
 */

import { MapMode } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { Trash2 } from "lucide-react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactElement } from "react";
import { useCallback, useEffect, useRef, useState } from "react";

import type { IconComponent } from "../components/primitives";
import { FIELD, IconButton } from "../components/primitives";
import type { FloatingContext } from "./floatingContext";
import { watchDocChanges } from "./floatingContext";
import {
  ImageCenterIcon,
  ImageFloatLeftIcon,
  ImageFloatRightIcon,
  ImageFullIcon,
  ImageOriginalIcon,
  ImageWidth25Icon,
  ImageWidth50Icon,
} from "./imageIcons";
import type { ImageFormat } from "./imageFormat";
import { altDisplay } from "./pictures/altText";
import { pictureAt } from "./pictures/refs";
import type { PictureAlign, PictureSize } from "./pictures/verbs";
import {
  removePicture,
  setPictureAlign,
  setPictureAlt,
  setPictureSize,
} from "./pictures/verbs";

/** The bar's context on a picture. */
export type PictureContext = Extract<
  NonNullable<FloatingContext>,
  { kind: "image" }
>;

/** What the bar gives each control for its one roving tab stop. */
export interface RovingProps {
  "data-roving": string;
  tabIndex: number;
  onFocus: () => void;
}

/** The line that stands in for size and place on a picture not attached. */
const NOT_ATTACHED = "Size and place can be set only for attached pictures.";

const ALT = "Alt text";
const REMOVE = "Remove picture";

const PLACES: { label: string; align: PictureAlign; icon: IconComponent }[] = [
  { label: "Left with text wrap", align: "left", icon: ImageFloatLeftIcon },
  { label: "Centered", align: "center", icon: ImageCenterIcon },
  { label: "Right with text wrap", align: "right", icon: ImageFloatRightIcon },
];

const SIZES: { label: string; size: PictureSize; icon: IconComponent }[] = [
  { label: "Small", size: "small", icon: ImageWidth25Icon },
  { label: "Medium", size: "medium", icon: ImageWidth50Icon },
  { label: "Full width", size: "full", icon: ImageFullIcon },
  { label: "Original size", size: "original", icon: ImageOriginalIcon },
];

/** The controls' names in their roving order. */
// eslint-disable-next-line react-refresh/only-export-components
export function pictureLabels(formattable: boolean): string[] {
  return [
    ...(formattable
      ? [...PLACES.map((p) => p.label), ...SIZES.map((s) => s.label)]
      : []),
    ALT,
    REMOVE,
  ];
}

/** Whether `format` reads as the placement `align`; a full picture has none. */
function placed(format: ImageFormat, align: PictureAlign): boolean {
  return format.align === align;
}

/** Whether `format` reads as the preset `size`; any other width is none. */
function sized(format: ImageFormat, size: PictureSize): boolean {
  switch (size) {
    case "small":
      return format.align !== "full" && format.width === "25%";
    case "medium":
      return format.align !== "full" && format.width === "50%";
    case "full":
      return format.align === "full";
    case "original":
      return format.align !== "full" && format.width === undefined;
  }
}

function isDestroyed(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
}

/** Where the picture the field belongs to stands now, and its text then. */
interface Place {
  from: number;
  to: number;
  text: string;
}

function placeOf(main: EditorView, context: PictureContext): Place {
  return {
    from: context.from,
    to: context.to,
    text: main.state.sliceDoc(context.from, context.to),
  };
}

const DIVIDER = (
  <span
    aria-hidden="true"
    className="mx-0.5 h-5 w-px shrink-0 bg-slate-200 dark:bg-slate-700"
  />
);

export function PictureControls({
  main,
  context,
  roving,
}: {
  main: EditorView;
  context: PictureContext;
  roving: (label: string) => RovingProps;
}): ReactElement {
  /** Bumped on every change of main's text, so the bar draws what it says now. */
  const [, setTick] = useState(0);
  /** The alt text being typed, or null while the field shows the picture's. */
  const [draft, setDraft] = useState<string | null>(null);
  const typed = useRef<string | null>(null);
  const [first] = useState(() => placeOf(main, context));
  /** The field's picture, or null once it is gone. */
  const place = useRef<Place | null>(first);
  const mounted = useRef(true);

  /**
   * Writes a changed field to its picture, once, if the picture is still
   * there. The selection is left as it is: a close may have moved it on.
   */
  const commit = useCallback(() => {
    const next = typed.current;
    if (next === null) return;
    typed.current = null;
    if (mounted.current) setDraft(null);
    const at = place.current;
    if (at === null || isDestroyed(main)) return;
    const ref = pictureAt(main.state, at.from + 1);
    if (ref?.from !== at.from || ref.to !== at.to) return;
    setPictureAlt(main, next, at.from + 1, false);
  }, [main]);

  // The field's picture, followed through every change of the text.
  useEffect(() => {
    mounted.current = true;
    const stop = watchDocChanges(main, (changes) => {
      const at = place.current;
      if (at !== null) {
        if (changes === null) {
          // A rebuilt state: the picture is there only as it was written.
          if (main.state.sliceDoc(at.from, at.to) !== at.text) {
            place.current = null;
          }
        } else {
          const from = changes.mapPos(at.from, 1, MapMode.TrackAfter);
          const to = changes.mapPos(at.to, -1, MapMode.TrackBefore);
          place.current =
            from === null || to === null || to <= from
              ? null
              : { from, to, text: main.state.sliceDoc(from, to) };
        }
      }
      if (mounted.current) setTick((n) => n + 1);
    });
    return () => {
      mounted.current = false;
      // Written after React's run: a close can come from inside the
      // editor's own update, which takes no dispatch.
      queueMicrotask(() => {
        commit();
        stop();
      });
    };
  }, [main, commit]);

  // The bar stands by another picture now: a change typed for the last one
  // goes to it, and the field follows the new one. The context is the
  // picture's selection, mapped through the same changes as the field's
  // place, so the same picture always reads as the same place.
  const { from: contextFrom, to: contextTo } = context;
  useEffect(() => {
    const at = place.current;
    if (at?.from === contextFrom && at.to === contextTo) return;
    commit();
    place.current = {
      from: contextFrom,
      to: contextTo,
      text: main.state.sliceDoc(contextFrom, contextTo),
    };
  }, [main, contextFrom, contextTo, commit]);

  /** Runs `verb` on the picture, a typed alt text written first. */
  const run = (verb: (view: EditorView, pos: number) => boolean) => {
    commit();
    verb(main, (place.current?.from ?? context.from) + 1);
    main.focus();
  };

  const ref = pictureAt(main.state, context.from + 1);
  const picture = ref?.from === context.from ? ref : null;
  const format = picture?.format ?? null;
  const shown =
    picture === null
      ? ""
      : altDisplay(main.state.sliceDoc(picture.altFrom, picture.altTo));

  const onFieldKey = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      commit();
      main.focus();
    } else if (event.key === "Escape") {
      // Put back before the focus leaves, so the blur writes nothing; the
      // bar then hands the focus back.
      typed.current = null;
      setDraft(null);
    }
  };

  return (
    <>
      {context.formattable && format !== null ? (
        <>
          {PLACES.map(({ label, align, icon }) => (
            <IconButton
              key={label}
              label={label}
              icon={icon}
              aria-pressed={placed(format, align)}
              onClick={() => {
                run((view, pos) => setPictureAlign(view, align, pos));
              }}
              {...roving(label)}
            />
          ))}
          {DIVIDER}
          {SIZES.map(({ label, size, icon }) => (
            <IconButton
              key={label}
              label={label}
              icon={icon}
              aria-pressed={sized(format, size)}
              onClick={() => {
                run((view, pos) => setPictureSize(view, size, pos));
              }}
              {...roving(label)}
            />
          ))}
        </>
      ) : (
        <span className="px-1.5 text-xs text-slate-600 dark:text-slate-300">
          {NOT_ATTACHED}
        </span>
      )}
      {DIVIDER}
      <input
        type="text"
        aria-label={ALT}
        placeholder="Describe the picture"
        value={draft ?? shown}
        spellCheck
        className={`${FIELD} w-44`}
        onChange={(event) => {
          typed.current = event.target.value;
          setDraft(event.target.value);
        }}
        onKeyDown={onFieldKey}
        onBlur={commit}
        {...roving(ALT)}
      />
      <IconButton
        label={REMOVE}
        icon={Trash2}
        onClick={() => {
          run(removePicture);
        }}
        {...roving(REMOVE)}
      />
    </>
  );
}
