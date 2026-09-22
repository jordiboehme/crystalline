# Virtual domains

A virtual domain keeps its engrams in the database, for the cases where a folder is baggage.

Most domains are folders of files. A virtual domain is the other option: its engrams live in the database, with no filesystem root. Reach for one where a filesystem is baggage rather than a feature: a container with no writable volume, a PostgreSQL backend shared across machines, or a domain you would rather not mirror to disk at all.

```sh
# Register a database-backed domain and scaffold its MANIFEST into the index.
crystalline domain add decisions --virtual

# It works with the same tools as any domain.
crystalline write decisions "First decision" --content "captured straight into the database"
crystalline search "captured"
```

Unregistering one is the one removal that deletes knowledge, since there is no folder left behind: `crystalline domain remove decisions --purge`, `?purge=true` on the JSON API and `purge: true` on the `remove_domain` tool all say the same thing. Without it the removal refuses and says so. Export first if you want a copy.

Two commands move engrams between the two kinds of truth:

- `crystalline domain import <path> --domain <name>` loads already-well-formed engram files into a virtual domain, verbatim. It is distinct from `crystalline import`, which converts a legacy tree into a *file* domain's directory.
- `crystalline domain export <path> --domain <name>` writes any domain's engrams back out as a normal markdown folder. This is how you take a virtual domain's data out to run `crystalline verify` on it, or convert it back to files whenever you change your mind.

Concurrent edits to the same virtual engram are guarded: `read_engram` returns a checksum, and passing it back as `expected_checksum` on `edit_engram` refuses the edit if the engram changed since you read it. A stale write conflicts instead of clobbering. Omit it for last-write-wins.
