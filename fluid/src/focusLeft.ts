/**
 * Did focus really leave a group of controls?
 *
 * For the `onBlur` of a two-step confirm and anything alike that closes
 * itself when the person goes elsewhere. Safari and WebKit do not focus a
 * button when it is clicked: on mousedown the focus moves to the nearest
 * focusable ancestor instead, which in this app is the page's `<main
 * tabIndex={-1}>`. A `focusout` handler that reads that as "left the group"
 * closes the confirmation before mouseup, the confirm button unmounts and its
 * click never fires. Chromium focuses the button, so only WebKit shows it.
 *
 * Four answers, in this order:
 * - no destination (`null`): not leaving. That is what a click looks like
 *   mid-flight, and it was never a reason to close;
 * - a destination that contains the group (an ancestor, as Safari's click
 *   lands on): not leaving, the pointer is still inside the group;
 * - a destination inside the group: not leaving;
 * - anything else outside the group: leaving.
 */
export function focusLeft(
  group: Node,
  next: EventTarget | null | undefined,
): boolean {
  if (!(next instanceof Node)) return false;
  if (group.contains(next)) return false;
  if (next.contains(group)) return false;
  return true;
}
