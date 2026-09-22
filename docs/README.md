# Crystalline docs

The [Crystalline Handbook](https://jordiboehme.github.io/crystalline/) is the long read; these pages are the reference, one per topic.

| Page | What it covers |
|---|---|
| [Claude Code](setup/claude-code.md) | `crystalline install claude-code`: what it wires, its flags and how to undo it |
| [Claude Desktop](setup/claude-desktop.md) | The one-click extension and the companion skill |
| [Codex CLI](setup/codex.md) | `crystalline install codex` and what to do after an upgrade |
| [GitHub Copilot CLI](setup/copilot.md) | `crystalline install copilot`, where its hooks and skills land, and what to do after an upgrade |
| [Any MCP harness](setup/mcp-harness.md) | `crystalline mcp` over stdio, wired by hand, and the daemon behind it |
| [Remote clients](setup/remote-clients.md) | A standing instruction for chat surfaces and the Messages API connector |
| [From the terminal](setup/terminal.md) | The CLI walkthrough: a domain, an engram, a search |
| [How an agent learns](learning-loop.md) | Domains and engrams, session onboarding, the learning loop, the tools, skills and provisioning |
| [Fluid, the web UI](fluid.md) | What a person does in the browser |
| [Teams](teams.md) | Team domains on GitHub, private domains and review mode |
| [Verify, evolve and doctor](evolve.md) | The three checks, the GitHub Action, the importer and the generated listings |
| [Virtual domains](virtual-domains.md) | Engrams that live in the database, with no folder behind them |
| [Architecture](architecture.md) | The crates, the daemon and the one source of truth |
| [Deployment](deployment.md) | Every scenario with a diagram, the container image and the environment variables |
| [FAQ](faq.md) | Short answers, and why not just a folder of files |

## What to say

Every step is a conversation. The rows use the two domains from the README's Get started, `books` and `work`.

| To do this | Say to your agent |
|---|---|
| Commission a domain | "Create a new Crystalline domain called books for what I read." |
| Retire a whole domain | "We are done with the work domain - unregister it, the files can stay." |
| Capture a fact | "Learn this: the deploy script needs the staging flag first." |
| Recall, scoped | "What do I like in a novel, going by books?" |
| Recall, everywhere | "Any single points of failure we should worry about?" |
| Walk the graph | "Walk out from the deploy decision and show what connects." |
| Recall what was true then | "Which deploy setup applied last June?" |
| Catch up | "What changed while I was away?" |
| Ingest a source | "Read this release page and capture only what affects us." |
| Correct a fact | "Update the timeout value, do not start a new engram." |
| Retire a fact | "The old queue setup is retired - supersede it, keep why." |
| Split before retiring | "That routine is over, but the backup step still holds - split it out first." |
| Tidy vocabulary | "Have our database tags drifted?" |
| Ask what needs work | "Sweep work and tell me what the archive needs." |
| Share with the team | "Share the deploy findings as a proposal for review." |
| Share part of it | "Share only the deploy engram to work and keep the rest local." |
| Answer a review | "Spell out the timeout value and update proposal 1." |
| Withdraw a proposal | "Withdraw the work proposal and keep my local edits." |
| Turn on review mode | "Put work in review mode - every write should land as a private draft." |
| Turn off review mode | "Take work out of review mode, fold Bob's draft and drop mine." |
