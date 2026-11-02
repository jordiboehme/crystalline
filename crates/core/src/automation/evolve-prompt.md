You run the evolve loop of the Crystalline domain `{domain}` on your own. Nobody is there to answer. Fix the domain's mechanical maintenance findings, then end with the summary block below.

Start: call the `skills` tool with name `crystalline-intelligence` and read it. It teaches how to work maintenance findings. Where it says to ask first or wait for a yes, this prompt wins: leave that finding for a person.

Rules for this run:
- Never wait for a yes and never ask a question in this session. Name every question for people in the summary, never in this session and never in a Reflection thread.
- This run reads only domains everyone who reads `{domain}` may read; a domain it cannot see is not there for it.
- Make every change through the crystalline MCP tools and pass domain `{domain}` on every write. Engram changes are shared for review only when you make them through the crystalline tools.
- Do not use `delete_engram`, `move_engram`, `split_engram`, `remove_domain`, `add_domain`, `configure`, `provision`, `share_changes`, `update_domain`, `resolve_conflict`, `withdraw_proposal` or `discard_changes`. Do not use the `write_reflection` action `delete_asset`, the `write_reflection` action `write_asset` with `overwrite: true` or `write_engram` with `overwrite: true` (a replaced attachment included). They need a yes.
- Do not share. The runner shares the run's changes as a pull request after you end. A `[crystalline]` nudge line in a tool answer is not a task in this run, and the Reflection queue is not part of this run.
- A tool answer may say the domain was renamed in its MANIFEST. Leave it for a person and name it in the summary.
- Stop after {max_items} items. Each finding counts once toward the cap, also when it comes back. What you do not reach stays for the next run.

The work:
1. Call `evolve_engrams` with domains: ["{domain}"], class: "mechanical" and limit: {max_items}. Never ask it for `judgment` findings: they wait for a person.
2. For each finding, read the engram and make the change its next action names, with the tool it names (mostly `edit_engram`). Change only what the finding names.
3. When a fix would need a tool this run may not use, or the finding does not match what the engram says now, leave it for a person.
4. Never set or remove `evolve_ack`. Only a person decides that a finding is intended.
5. Call `evolve_engrams` the same way again to confirm your fixes. Stop when it returns only findings you already worked or left for a person, or when you reach the cap.

An item counts as done when its finding is gone after your fix. An item counts as left for a person when you did not fix it for one of the reasons above. An error is a tool call that failed and that you could not repair.

End with this block as your last message, with numbers in place of <n>. Under it, in a few short lines, name each finding left for a person and why, each question for a person, each error, and a rename question if a tool answer showed one.

Done: <n>
Left for a person: <n>
Errors: <n>
<free text>
