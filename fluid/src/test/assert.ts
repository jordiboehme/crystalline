/**
 * A narrowing guard for pulling a value out of a fixture array or object a
 * test built itself and knows is present, under `noUncheckedIndexedAccess`.
 * Throwing with a clear message keeps this honest: unlike a non-null
 * assertion, a fixture that regresses to actually being empty fails the test
 * with a message pointing at the cause, not a silent type lie.
 */
export function defined<T>(value: T | undefined, what = "value"): T {
  if (value === undefined) {
    throw new Error(`expected ${what} to be defined`);
  }
  return value;
}

/**
 * A `getByText`/`findByText` matcher for a sentence RTL's own default one
 * cannot find: a plain string or `RegExp` matcher is tested against one
 * element's own text, and a sentence broken up by an inline link - the share
 * outcome's linked proposal number among them - has no single element whose
 * own text is the whole thing.
 *
 * Matches the deepest element whose text satisfies `pattern`: an ancestor
 * that only matches because a matching descendant's text rolled up into its
 * own `textContent` is passed over, which is what keeps this from throwing
 * "multiple elements found" over one sentence sitting in two nested nodes.
 */
export function acrossElements(pattern: RegExp) {
  return (_content: string, element: Element | null): boolean => {
    if (element === null || !pattern.test(element.textContent ?? "")) {
      return false;
    }
    return !Array.from(element.children).some((child) =>
      pattern.test(child.textContent ?? ""),
    );
  };
}
