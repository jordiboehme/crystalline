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
