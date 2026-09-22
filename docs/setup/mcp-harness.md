# Any MCP harness

The server command is always `crystalline mcp`; everything the installer does can be done by hand.

Crystalline runs as an MCP server over stdio:

```sh
claude mcp add crystalline --scope user -- crystalline mcp --harness claude-code
codex mcp add crystalline -- crystalline mcp --harness codex
copilot mcp add crystalline -- crystalline mcp --harness copilot
```

`--harness` is optional and tells the server which harness spawned it, so a harness that already has the skills installed as files is not served them a second time over MCP (see [Skills over MCP](../learning-loop.md#skills-over-mcp)). Leave it out and the full surface is served. The `--` matters on the Claude Code line: without it, `claude mcp add` reads the server's own flags as its options.

The first agent to connect starts a background daemon that loads the embedding model once and watches every registered domain. Every later connection - other agents, other terminals, other harnesses - attaches to that same daemon, so there is always one shared instance and one consistent view of the index. The Claude Desktop extension is the one exception: its daemon runs from inside Desktop's extension folder and leaves on its own shortly after Desktop does (see [Personal workstation](../deployment.md#personal-workstation)). A daemon running in a container is reached over HTTP instead of stdio - see [Run in a container](../deployment.md#run-in-a-container).
