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

Picoschema syntax, field by field:

- `name: type` - a required field of that scalar type (`string`, `integer`, `number`, `boolean`, `any`).
- `name?: type` - the trailing `?` makes it optional.
- `name(enum):` followed by a YAML list - a closed set of allowed string values.
- `name(array): type` - a list of that type; combine with `?` for an optional array (`name?(array): type`).
- A **Capitalized** type name (`Decision`, `Task`, `Note`) declares a relation to another engram type rather than a scalar - so `supersedes?: Decision` means an optional `- supersedes [[Some Decision]]` relation, not a string field.
- Everything under `schema:` describes the engram's body (observation categories and relations); everything under `settings.frontmatter` describes expected frontmatter fields using the exact same syntax.
- `settings.validation` controls severity: `warn` (the default - issues are warnings), `strict` (issues become errors), or `off` (schema is not enforced at all).

## Bootstrap from what already exists

Rather than guessing a shape from scratch, generalize one from engrams already captured under a `type`:

```json
{ "tool": "infer_schema", "arguments": { "domain": "engineering", "type": "decision", "threshold": 0.25 } }
```

`threshold` is the frequency at or above which a field is suggested at all (0.25 by default: present in at least a quarter of the engrams); a field present in 95% or more is suggested as required rather than optional. Lower the threshold to surface more of a domain's organic vocabulary, or raise it to keep the suggested schema to only its strongest patterns. Treat the result as a starting draft, not a final schema - trim fields that do not deserve to be required, and turn any inferred relation target into a properly Capitalized type name before saving it as a schema engram.

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

## Enforcing it in CI

`crystalline verify` reads the same schema engrams and reports the same conformance issues (rule family `S`) as part of its static rule catalog - no database, service or network connection required. A domain's schemas are enforced automatically wherever `crystalline verify` (or the `crystalline verify` GitHub Action) already runs against that domain; `--strict` promotes every warning-level rule, including schema conformance warnings, to an error that fails the check.
