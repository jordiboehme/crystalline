---
name: crystalline-schema
description: Use when a Crystalline domain wants structure for one of its engram types - authoring a Picoschema schema engram, running infer_schema to bootstrap one from what is already captured, or validate_engrams to check conformance.
---

# Crystalline Schema

A domain's engrams need no schema by default - `type` and `status` are free-form strings. Reach for a schema engram once a domain wants a given `type` to carry a consistent shape: required observation categories, required relations, or expected frontmatter fields.

## Authoring a schema engram

A schema is itself an engram, `type: schema`, whose frontmatter declares the shape for another `type` via `entity`. Body content is optional prose describing the schema; the shape lives entirely in frontmatter.

```yaml
---
type: schema
title: Decision Schema
tags:
  - schema
  - engineering
status: current
entity: decision
version: 1
schema:
  summary: string, one line summary of the decision
  rationale?: string
  priority(enum):
    - low
    - medium
    - high
  alternative?(array): string
  supersedes?: Decision
settings:
  validation: warn
  frontmatter:
    status(enum):
      - current
      - proposed
      - superseded
    owner: string
---

# Decision Schema

Engrams of type `decision` in the engineering domain follow this shape.
```

That is the on-disk shape. Through the MCP write tool none of that frontmatter goes into `content` - `write_engram` builds the frontmatter itself from its arguments, and `entity`, `version`, `schema` and `settings` travel through `metadata` as an object:

```json
{
  "tool": "write_engram",
  "arguments": {
    "domain": "engineering",
    "title": "Decision Schema",
    "type": "schema",
    "tags": ["schema", "engineering"],
    "metadata": {
      "entity": "decision",
      "version": 1,
      "schema": {
        "summary": "string, one line summary of the decision",
        "priority(enum)": ["low", "medium", "high"]
      },
      "settings": { "validation": "warn" }
    },
    "content": "The shape for decision engrams.\n\n- [convention] Decisions carry a summary and a priority #schema\n- [convention] Adopted in warn mode first #schema"
  }
}
```

Never embed a second `---` frontmatter block inside `content`: that duplicates the frontmatter the tool already generates and yields a schema with no entity and no declarations. Keep `content` to plain prose and bullets - a schema is an engram like any other and needs at least 3 non-blank content lines to pass verification.

Picoschema syntax, field by field:

- `name: type` - a required field of that scalar type (`string`, `integer`, `number`, `boolean`, `any`).
- `name?: type` - the trailing `?` makes it optional.
- `name(enum):` followed by a YAML list - a closed set of allowed string values.
- `name(array): type` - a list of that type; combine with `?` for an optional array (`name?(array): type`).
- A **Capitalized** type name (`Decision`, `Task`, `Note`) declares a relation to another engram type rather than a scalar - so `supersedes?: Decision` means an optional `- supersedes [[Some Decision]]` relation, not a string field.
- Everything under `schema:` describes the engram's body (observation categories and relations); everything under `settings.frontmatter` describes expected frontmatter fields using the exact same syntax.
- `settings.validation` controls severity: `warn` (the default - issues are warnings), `strict` (issues become errors), or `off` (schema is not enforced at all).
- `entity` is always the lowercase `type` string of the engrams it governs; Capitalized names appear only as relation targets inside `schema:` values. Do not swap these.
- Declare fields under exactly the names the user asked for - no pluralizing, no paraphrasing - and add no `settings.frontmatter` constraints nobody requested. A new schema immediately re-checks every existing engram of its type, so an invented constraint manufactures violations out of thin air.

## Bootstrap from what already exists

Rather than guessing a shape from scratch, generalize one from engrams already captured under a `type`:

```json
{ "tool": "infer_schema", "arguments": { "domain": "engineering", "type": "decision", "threshold": 0.25 } }
```

`threshold` is the frequency at or above which a field is suggested at all (0.25 by default: present in at least a quarter of the engrams); a field present in 95% or more is suggested as required rather than optional. Lower the threshold to surface more of a domain's organic vocabulary, or raise it to keep the suggested schema to only its strongest patterns.

The result is a draft in the tool's own JSON shape, not finished Picoschema. Translate it into declarations - plain `name: type` lines, `?` for optional, `(enum)` and `(array)` modifiers - before saving through `metadata`; pasting the raw draft in produces malformed declarations that fail verification. While translating, apply judgment the heuristic lacks: trim fields that do not deserve to be required, promote a field the strong majority shares when the outliers are a different kind of engram, and turn any inferred relation target into a properly Capitalized type name.

## Checking conformance

Validate a domain's engrams against whatever schema engrams it has:

```json
{ "tool": "validate_engrams", "arguments": { "domain": "engineering" } }
```

Narrow to one engram or one type when iterating on a schema change:

```json
{ "tool": "validate_engrams", "arguments": { "domain": "engineering", "type": "decision" } }
```

```json
{ "tool": "validate_engrams", "arguments": { "domain": "engineering", "identifier": "adopt-postgres" } }
```

Issues report as warnings or errors depending on `settings.validation` for the matched schema; a missing required observation, an out-of-enum value or a missing required relation are typical findings. Adopt a new schema with `settings.validation: warn` first, watch what it flags across the domain's existing engrams, and only promote to `strict` once the domain's engrams have actually converged on the shape.

When the report comes back with `schemas: 0`, say plainly that no schema governs the domain before mentioning the zero-issue count - "all engrams conform" implies a validation that never ran.

## Writing under a schema

Before writing an engram of a type a domain governs, read that type's schema and match its required shape literally: a required `objective` observation means a bullet tagged `[objective]`, not a synonym or plain prose covering the same ground, and required frontmatter fields go through `metadata`. Under `strict` a nonconforming write lands as a verify error, not a warning.

## Enforcing it in CI

`crystalline verify` reads the same schema engrams and reports the same conformance issues (rule family `S`) as part of its static rule catalog - no database, service or network connection required. A domain's schemas are enforced automatically wherever `crystalline verify` (or the `crystalline verify` GitHub Action) already runs against that domain; `--strict` promotes every warning-level rule, including schema conformance warnings, to an error that fails the check.
