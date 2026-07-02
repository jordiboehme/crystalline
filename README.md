# Crystalline

[![CI](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml/badge.svg)](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml)

Local-first knowledge management for humans and AI agents.

## What is Crystalline

Crystalline organizes knowledge as plain markdown files called Engrams, grouped into folders called Domains. Every Domain carries a MANIFEST.md that describes its scope and when it should be used. The files on disk are always the source of truth; Crystalline builds a local, disposable database on top of them as a derived index for fast search and graph traversal. A command-line tool and an MCP server sit on top of that index, so both people and AI agents can read, write and search the same knowledge base through the same rules.

Status: early development. The on-disk format, storage layer and MCP tool surface are still being built out; expect breaking changes before a 1.0 release.

## Planned features

- Markdown Engrams as the single source of truth, with the database rebuilt from files on demand
- An embedded index combining hybrid text search and semantic search
- An MCP server that gives AI agents structured read, write and search access to your knowledge
- Static verification for CI, so malformed knowledge fails a pull request instead of a query
- Routing prompt generation, so a session can be pointed at the right Domains automatically

## Build from source

```sh
git clone https://github.com/jordiboehme/crystalline.git
cd crystalline
cargo build --release
```

The resulting binary is at `target/release/crystalline`.

## License

GNU Affero General Public License v3.0 - see [LICENSE](LICENSE)
