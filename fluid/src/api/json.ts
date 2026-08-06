/**
 * Reading fields off an untyped payload.
 *
 * Several endpoints pass the engine's own JSON through unchanged, which is the
 * point (the MCP tools and this API answer with one payload rather than two
 * shapes that drift) and also means the OpenAPI document types them as opaque
 * objects. Every reader in `api/` asserts its shape by reading it rather than
 * by casting it, so a field that is missing or of a different type is dropped
 * instead of turning into a `TypeError` three components deep. These are the
 * primitives they all read with.
 */

/** A JSON object, for reading fields off an unknown payload. */
export type JsonObject = Record<string, unknown>;

/** The value as an object, or null when it is anything else. */
export function asObject(value: unknown): JsonObject | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

/** The value as an array, or an empty one when it is anything else. */
export function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

/** The value as a non-empty string, or null. */
export function asString(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

/** The strings in an array, dropping everything else. */
export function asStrings(value: unknown): string[] {
  return asArray(value).filter(
    (item): item is string => typeof item === "string",
  );
}

/** The value as a finite number, or null. */
export function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}
