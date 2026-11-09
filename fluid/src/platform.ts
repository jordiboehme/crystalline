/**
 * Which keyboard this is, and how to name its keys.
 *
 * One test for the whole app, the one CodeMirror's keymap uses: a key's label
 * and what `Mod` does never disagree. It is read once, when this module
 * loads, as CodeMirror reads it once. A test that wants a Mac sets
 * `navigator.platform` in `vi.hoisted`, before any import.
 */

const MAC = ((): boolean => {
  if (typeof navigator === "undefined") return false;
  // CodeMirror's own test (@codemirror/view `browser.mac`): a platform that
  // says Mac, or Safari on an iPhone or iPad, which may say MacIntel too.
  const safari = /Apple Computer/.test(navigator.vendor);
  const ios =
    safari &&
    (/Mobile\/\w+/.test(navigator.userAgent) || navigator.maxTouchPoints > 2);
  return ios || /Mac/.test(navigator.platform);
})();

/** Whether this is a Mac (or an iPhone or iPad) keyboard. */
export function isMac(): boolean {
  return MAC;
}

/**
 * The key that opens the menu of the item the keyboard is on: the menu key,
 * Shift+F10, and on a Mac Ctrl+Return too, the keyboard twin of a Ctrl+click.
 * A Mac has no menu key, and F10 needs fn on a MacBook.
 *
 * Ctrl+Return counts on a Mac only: elsewhere Ctrl+Enter is `Mod-Enter`, which
 * flips a task.
 */
export function isMenuKey(event: KeyboardEvent): boolean {
  if (event.altKey || event.metaKey) return false;
  if (event.key === "ContextMenu") return !event.ctrlKey;
  if (event.key === "F10") return event.shiftKey && !event.ctrlKey;
  return MAC && event.key === "Enter" && event.ctrlKey && !event.shiftKey;
}

/** Mac modifiers in Apple's order, with their sign and their word. */
const MAC_MODS: [string, string, string][] = [
  ["Ctrl", "⌃", "Ctrl"],
  ["Alt", "⌥", "Option"],
  ["Shift", "⇧", "Shift"],
  ["Meta", "⌘", "Cmd"],
];

const OTHER_MODS: [string, string][] = [
  ["Ctrl", "Ctrl"],
  ["Meta", "Win"],
  ["Alt", "Alt"],
  ["Shift", "Shift"],
];

/** Key names that read differently from CodeMirror's: [Mac sign, Mac word, other]. */
const KEYS: Record<string, [string, string, string]> = {
  Enter: ["↩", "Return", "Enter"],
  Backspace: ["⌫", "Delete", "Backspace"],
  Delete: ["⌦", "Forward Delete", "Delete"],
  Escape: ["Esc", "Esc", "Esc"],
  Tab: ["⇥", "Tab", "Tab"],
  ArrowUp: ["↑", "Up Arrow", "Up Arrow"],
  ArrowDown: ["↓", "Down Arrow", "Down Arrow"],
  ArrowLeft: ["←", "Left Arrow", "Left Arrow"],
  ArrowRight: ["→", "Right Arrow", "Right Arrow"],
  Space: ["Space", "Space", "Space"],
  " ": ["Space", "Space", "Space"],
};

/**
 * A CodeMirror key name ("Mod-Shift-z", "Alt-F10", "Ctrl-Enter") as this
 * keyboard calls it. On a Mac "⌘⇧Z", or with `words` (for a sentence or a
 * screen reader) "Shift+Cmd+Z"; elsewhere "Ctrl+Shift+Z" either way.
 */
export function keyLabel(cmKey: string, { words = false } = {}): string {
  // The last part is the key, and may itself be "-".
  const parts = cmKey.split(/-(?!$)/);
  const key = parts.pop() ?? "";
  const held = new Set<string>();
  for (const part of parts) {
    if (/^(Mod)$/i.test(part)) held.add(MAC ? "Meta" : "Ctrl");
    else if (/^(Cmd|Meta|m)$/i.test(part)) held.add("Meta");
    else if (/^(Ctrl|Control|c)$/i.test(part)) held.add("Ctrl");
    else if (/^(Alt|a)$/i.test(part)) held.add("Alt");
    else if (/^(Shift|s)$/i.test(part)) held.add("Shift");
  }
  const known = KEYS[key];
  const name = key.length === 1 ? key.toUpperCase() : key;
  if (MAC && !words) {
    const signs = MAC_MODS.filter(([mod]) => held.has(mod)).map(
      ([, sign]) => sign,
    );
    return signs.join("") + (known?.[0] ?? name);
  }
  if (MAC) {
    const said = MAC_MODS.filter(([mod]) => held.has(mod)).map(
      ([, , word]) => word,
    );
    return [...said, known?.[1] ?? name].join("+");
  }
  const said = OTHER_MODS.filter(([mod]) => held.has(mod)).map(
    ([, word]) => word,
  );
  return [...said, known?.[2] ?? name].join("+");
}
