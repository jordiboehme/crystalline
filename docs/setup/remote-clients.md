# Remote clients

A chat surface or a Messages API agent runs no hooks, so it onboards itself with one standing instruction.

A remote service or a chat harness runs no session hooks, and most of them never show the model an MCP server's instructions, so neither onboarding path in [Session onboarding](../learning-loop.md#session-onboarding) reaches the agent. Give it a standing instruction instead: paste this into the client's custom instructions and the agent onboards itself with one tool call at the start of every session.

```text
This environment includes Crystalline, your crystallized intelligence across sessions, over MCP. At the start of every session call its list_domains tool with include_routing set to true; the result is your onboarding: one routing line per domain plus the behavior rules for this server's tools. Follow it, search those domains before answering from memory and re-fetch it mid-session with the same call whenever you need it again.
```

`crystalline prompt connector` prints the same snippet, ready to copy. The same text is also available in-client, with no copy-paste, as the `connector` MCP prompt; a harness that shows the model MCP prompts can insert the `onboarding` prompt directly instead, which carries the live routing block itself rather than the instruction to fetch it (see [Skills over MCP](../learning-loop.md#skills-over-mcp)).

An agent built on the Messages API MCP connector can keep its context lean by deferring most of the tool surface: with `defer_loading` on, a tool is declared but its description and schema load only when the model searches for it. Defer everything by default and pin the three tools an agent needs before it can search for anything - `search_engrams`, `read_engram` and `list_domains`, the trio that carries session onboarding and recall:

```json
{
  "mcp_toolset": {
    "type": "mcp_toolset",
    "mcp_server_name": "crystalline",
    "default_config": { "defer_loading": true },
    "configs": {
      "search_engrams": { "defer_loading": false },
      "read_engram": { "defer_loading": false },
      "list_domains": { "defer_loading": false }
    }
  }
}
```

Claude Code does this for you: it turns tool search on automatically once a session's MCP tool descriptions grow large, loading tool names plus each server's instructions up front and the rest on demand. The routing block is sized to survive that mode intact (see [Session onboarding](../learning-loop.md#session-onboarding)).
