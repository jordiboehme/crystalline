# The Reflection

A domain can keep a Reflection: documents for people, next to the engrams the agent keeps. This page explains how it works, who may do what, where it lives on a team and how an agent works on it every day.

## What a Reflection is

A Reflection is a domain's documents for people. They live in the folder `reflections/` of the domain, and each one is called a reflection. They are written at the level that `reflections/PACT.md` agrees on: for whom, how deep, how it is presented and in which structure.

Engrams stay the agent's knowledge. The Reflection is where people and agents meet. Both edit the pages, and both comment on them in threads. The agent brings what changed in the engrams into the pages, and it reads what people changed in the pages and decides what to learn from it.

A reflection is not an engram. It needs no frontmatter, `search_engrams` never returns it, and with the switch on the engram tools (`write_engram`, `edit_engram`, `move_engram`, `delete_engram`) refuse paths under `reflections/`. Four tools work on it instead: `reflect`, `read_reflection`, `write_reflection` and `discuss`. People read, edit and discuss the pages in Fluid, under the Reflection half of a domain's switch. Agents use the tools, and the files on disk and the JSON API work too.

## Switch it on

Set the MANIFEST key `reflection: on`. Do it in the domain policies card in Fluid, with `PATCH /api/v1/domains/{domain}/manifest`, or ask your agent (the `configure` tool with `domain`). Switching it on writes a first `reflections/PACT.md` with the four headings to fill in. Switching it off again is always allowed, and the files stay where they are. In Fluid the domain then opens on its Reflection. The Engrams half of the switch at the top shows the engrams.

On a domain connected to GitHub, switching it off has one more effect. The folder `reflections/` is then ordinary engram content, so "share changes" offers its pages, their threads and `PACT.md`. Threads can hold comments from people who may not see engrams. So do not share changes while the switch is off. Once these files are in the team's copy, switching on again is refused until the team's copy no longer holds them. The answer to the switch-off says this too.

If a page is moved while somebody has it open, Crystalline saves the page first, closes the editor and tells it the new path; the editor then offers to open the page there, with any text not saved yet. A rename of the domain does the same for every open page. If the Reflection is switched off while a page is open, Crystalline saves the page first and closes the editor. If the switch changes outside Crystalline (a plain editor, a pull), a page that is still open cannot be saved: the editor keeps its text, and when the last person leaves, the server keeps it in its state folder. `crystalline kept list` shows it and `crystalline kept show <id>` prints it, so you can copy it back (see [Kept texts](teams.md#kept-texts)).

Switching it on is refused in these cases, and the refusal says why:

- Engrams already live in `reflections/`. The refusal names them. Move them to another folder first (`move_engram`), then switch it on.
- The domain is virtual. A Reflection needs a domain kept as files.
- The domain is connected to GitHub and this instance has no `service.public_url`. A shared domain's Reflection has one home, and the home needs an address (see [One home, one backup](#one-home-one-backup)).
- The domain is connected to GitHub and the team's copy still holds files in `reflections/`. Share the change that moves them out first.
- The domain has two folders that differ only in letter case, for example `reflections/` and `Reflections/`.
- The domain is connected to GitHub and the record of the team's copy cannot be read. Repair or remove that record; the next pull writes it again.
- This instance's `service.public_url` cannot be a Reflection's home, for example because it is not a valid address for that.

A plain editor or a pull can still write `reflection: on` into the MANIFEST. If engrams still live in `reflections/` then, the switch is pending: the folder stays engram content until the last engram has left it, and the Reflection tools answer that they wait for that. A `reflect` call over all domains names such a domain under `scope.pending`. A page written by hand with no hidden section id counts as such an engram too (see the next sections). Two folders that differ only in letter case keep the switch pending the same way, until one of them is gone. So does an engram whose frontmatter no longer parses, or that an editor saved in another encoding than UTF-8: Crystalline still holds its last good version, so fix the file, then move it out. A file in the folder that never was an engram does not hold the switch. When the switch takes effect, however it got there (the domain policies, a plain editor, a pull, or the last engram leaving the folder), Crystalline reads the whole domain again at once: every page in `reflections/` leaves the engram index, and the Reflection lists all pages, their threads and files. Switching it off works the same way the other direction.

## The pact

`reflections/PACT.md` is the agreement between the people and the agents who work on the Reflection. It has five parts:

- `## Audience`: who reads these pages and what they already know.
- `## Level of detail`: how deep the pages go, and what they leave to the engrams.
- `## Presentation`: which diagrams, callouts, tables and status labels help these readers, and what to avoid.
- `## Structure`: one glob per top-level bullet, for example `- guides/*.md`. Only top-level bullets are read, so write the reasons in a sentence above the list.
- `## Pages`: one bullet per page or space a person asked for: its path or glob, what they asked for in their words, who asked and when. The agent adds the bullet when it is asked, and reads it to place new engrams or to recreate a page from the request.

The agent reads the pact before it writes. A page that fits none of the `## Structure` globs is reported to the agent as an update item. A change to the pact is reported too, so the agent applies it to the pages it affects. A `write_reflection` receipt names `unsourced_sections` (the headings) when the write created or changed a `##` section that has no sources, with a sentence asking the agent to pass them. It is a hint, never a refusal, and the pact is not a page, so it never reports it.

A `PACT.md` write's receipt names `structure_findings` when a `## Structure` bullet is not read the way it looks. Each finding has the `bullet` and a `reason`: it reads as a sentence, not a glob (a space and none of `*`, `?`, `[`, `/`), it is not a valid glob, it matches no page, or it is a nested bullet that holds a path (nested bullets are not read). When no glob fits any page, every `off_structure` item carries a `hint` that says how the bullets are read.

An agent's own writes never come back to it as feedback. A section an agent writes, changes or removes is reflected at once, with or without sources, on a page and in `PACT.md`, so an agent's pact write raises no `pact_changed`. A person's edit in Fluid, in the files or through the JSON API is still a feedback item (or `pact_changed`), also when an agent writes that section again before the change is settled.

## Presentation

A reflection is Markdown with a few extra elements: callouts, status labels, math, task items, footnotes and diagrams. The full list is in [The Markdown Crystalline understands](markdown.md). The `## Presentation` part of the pact narrows or extends the defaults, and where they differ the pact wins. A pact that says "no diagrams" means no diagrams. The skill `crystalline-presentation` teaches agents to read it first and to use an element only where it helps the reader.

Two rules matter for a reflection. Each `##` section renders on its own, so a footnote reference and its definition belong in the same section. And a change that only ticks or unticks a task item is not feedback, so a tick on the reading page does not send the agent back to the section.

## Sections and their hidden ids

Every `##` heading of a reflection gets a hidden id on the next line, for example `<!-- r:7f3a9c2e -->`. The id ties a section to its threads and to the engrams it was built from, so a section keeps them when its heading changes. Leave the line alone. A heading added in another editor gets its id on the next write through Crystalline.

The id lines also mark a file as a page. Give every page at least one `##` heading, and put an id line under it. A page written through Crystalline gets its id lines, but only under `##` headings, so a page without one has none. For an id you write by hand, use 4 to 16 lowercase hex digits that start with a letter, for example `<!-- r:c3d4e5f6 -->` (an id like `1234e567` can be read as a number). A file with no id line, no `reflection:` block in its frontmatter and no copy in the hidden folder `.reflected/` reads as an engram when Crystalline looks at the folder from scratch: when Reflection is switched on, after a rebuild such as `crystalline reindex --full`, and on the first sync of a new machine. The Reflection then waits until the file is moved out or marked.

The frontmatter block `reflection:` is Crystalline's own. For each section it records the engrams the section was built from (with their checksum at that time), a hash of the text at the last reflect, and who changed the text since then. A person who cannot read the engrams gets the page without this block, so engram names stay hidden from them. Do not edit the block by hand.

An agent can write a whole page or one section. `write_reflection` takes the same three section operations `edit_engram` takes: `replace_section`, `insert_before_section` and `insert_after_section`, with the section named by its heading. The section keeps its id. With `sources` and no text at all, the write curates the named sections as the page stands and rewrites none of its text. Crystalline still does its usual upkeep on that write: a heading without an id line gets one, a `crystalline://` address is spelled with the domain's current name, and the `reflection:` block records the sources. A write that leaves out a section a person changed since the last reflection still lands, and its receipt names that section. A heading that two sections share does not name either of them: a thread on it is opened by the section id. A `crystalline://` address in a page is spelled with the domain's current name on every write, as in an engram, and renaming a domain also respells its Reflection pages.

### Sources from other domains

A page can also be built from engrams of other domains. So one page can sum up several domains. In `sources`, name an engram of this domain by its permalink and an engram of another domain by its address, for example `crystalline://harbor/team/harbor-team`. An address of the page's own domain is the same as the permalink. Never write sources into the page as HTML comments or text: only `sources` lets the queue notice a change.

The writer needs the right to read engrams in the other domain. A domain the writer cannot see, a domain this instance does not have and a domain of a connected server are refused with the same words as a domain that does not exist. So a private domain never shows. The `reflection:` block stores the other domain by its name. Rename a domain with `crystalline domain rename`: it also renames the domain in the sources of every page, and a rename with `--local` keeps the old names as aliases, so the sources still resolve. A name changed only by editing `domain_name` in the MANIFEST is not followed. The sources that use the old name then read as `unavailable`.

A page may only cite a domain whose engrams everyone who reads the page may read too. Text on a page reaches every reader of the page, so a page must never carry text drawn from engrams fewer people may read. The two sides are counted differently. The page is read by the people who may read its domain's Reflection. The source is read by the people who may read its domain's engrams; a Reflection level there does not count, because it opens that domain's pages and never its engrams.

- A shared domain's Reflection is read by every anonymous visitor (counted even while the anonymous viewer is off), every account with the instance role `viewer`, `editor` or `admin`, every account with the role `none` that has an instance Reflection level, and every account created later. Its engrams are read by the same people, less the accounts with the role `none`.
- A private domain's Reflection is read by its owner, the instance admins, every member at knowledge `viewer` or more and every member at knowledge `none` with a Reflection level. Its engrams are read by the same people, less the members at knowledge `none`. Never by an anonymous visitor or a disabled account.

So a page of a shared domain never cites a private domain, and it cites another shared domain only while no account with the role `none` has an instance Reflection level. A page of a private domain cites a shared domain unless one of its readers is an account with the role `none`. It cites another private domain when every reader of the page also reads that domain's engrams. A source of the page's own domain is always fine. The machine owner and the admins read everything and make no difference.

The rule holds for every writer, the machine owner and a scheduled run included, on every stack: the daemon with its HTTP endpoint on or off, the embedded stdio server (`crystalline mcp --embedded`, and the fallback when no daemon answers) and a one-shot tool run. Each opens the accounts store before it answers anything. A stack that finds no store at all knows there is no account and no private domain yet; should the store appear while it runs, it treats the readers as unknown until it is restarted. When the store cannot be opened, nobody there knows who reads which domain, so every source outside the page's own domain is refused with the reason, and the queue names every such source, until the store opens again. A write that names a source fewer people may read is refused with "this source is in a domain fewer people may read than this page's domain; leave out the source and every sentence drawn from it, since the check sees only named sources, or write that text on a page in that domain instead", and nothing is written. A source in a domain the writer may not open is refused as before, as a domain that does not exist, so the refusal never names a domain the writer cannot see.

A page of a domain connected to GitHub is also backed up to that domain's repository, which is read by the people of the repository, not by the accounts here. So such a page may cite engrams of its own domain and of other domains connected to GitHub only. A source in a domain that is not connected to GitHub (a local or a virtual domain) is refused with "this source is in a domain that is not shared through GitHub, and this page is backed up to a GitHub repository; leave out the source and every sentence drawn from it, or cite it from a page in that domain instead". Personal knowledge never reaches a repository this way. This rule comes on top of the accounts rule: both must allow a source. The backup goes to the repository's `crystalline/reflections` branch, which every reader of that repository on GitHub can read, with an account here or without one. So with the GitHub App set up, a page may only cite another repository's domain when every reader of the page's repository may read that repository too: it is public, or the page's repository is private and each of its collaborators with read or more is also a collaborator there. Two domains on one repository always may cite each other. The App's cached list of collaborators answers this, and when that list is not known right now (GitHub is down for too long, or the App is not installed on one of the two repositories), the source is refused. Without the App, Crystalline does not know a repository's readers and treats any two connected domains alike. An existing page that breaks this rule, also because a repository got a new collaborator or was made public, gets the same `source_too_wide` item on the next queue scan and is held back from the backup like any other.

The rule sees only the sources a write names. It cannot read the text: an agent that leaves out a refused source and keeps the sentences it drew from it is not caught. So the refusal and the queue guidance ask for the text to go as well, and a curator who reviews a page built from several domains should read it with this in mind.

A file edited on disk, a page restored from the backup and a change of who may read a domain cannot be refused. The queue names each section that cites a domain fewer people may read as a `source_too_wide` item, also for pages written before this rule. The page keeps serving until a curator removes that source and the text drawn from it, because deleting text is a person's decision. Only a caller who may open that domain gets the item.

The backup branch is read by the people of the repository, not by the page's readers. So a page with a `source_too_wide` item is held back from the backup with its threads, its baseline and the files under `reflections/assets/` that only held pages link (a file another page links stays), and the branch head loses the copies it had. Its earlier versions stay in the branch history. The backup logs which pages it held back and looks again ten minutes later, or at once on the next change, so a settled page goes to the branch again. A change of who may read a domain marks nothing in the Reflection, so the daemon looks for pages that newly break the rule every five minutes, and right after every change of a domain's visibility, a membership or an account through the JSON API, and after `crystalline domain visibility`, `domain members` and `domain transfer` (which tell a running daemon), and schedules a backup that drops them off the branch head.

The queue notices when an engram of another domain changes, is retired or is gone, with the same items as for the page's own domain. When the other domain is not on this instance, or it is a domain of a connected server, its sources are `unavailable`. They are never `missing`, so no agent drops them by mistake. A write that sets a section's sources keeps every source the writer may not see. A writer who sees an unavailable source may leave it out of the section's sources, and that drops it: this is how a source of a domain that is gone for good goes. A section that is deleted takes all its sources with it, also those the writer could not see. Citing an engram of another domain does not count as covering it for that domain's own Reflection.

#### What an unattended run may read

The rule sees only the sources a write names, and a thread answer or a sentence written without sources names none. So the guard against a confused deputy sits on what an unattended agent may read. Anyone who may comment on a shared page can ask a question in a thread; an agent that runs on its own and could read a private domain would answer it from there into a page everyone reads, and no check on the answer could catch that.

So while a session is unattended, every read (search, `read_engram`, `build_context`, `browse_domain`, `recent_activity`, `list_domains`, the sources a page names, attachments, and the Reflection of other domains) answers as if the domains it may not carry onto its pages did not exist. A session is unattended when:

- it was started for a run: `crystalline reflect run` and `crystalline evolve run` start each agent with `crystalline mcp --unattended --domain <domain>`, or over HTTP with the header `X-Crystalline-Run-Domain: <domain>`;
- it was started with `--unattended` (mark your own automation this way, see below);
- or its caller holds a live lease of the reflect or evolve loop. For the machine owner that is any lease held without an account: on a local session the owner and the owner's own run look alike, so while such a run is going, the owner's other local sessions read the same way.

It may read a domain X only when every reader of the domain D it works on may read X (the rule above, GitHub half included: a run on a domain backed up to GitHub reads no local or virtual domain). D is the domain the session was started for, as long as the caller holds a live lease on it; a domain named without that lease counts for nothing, so naming one can only narrow what a session reads. A session that names no domain works on every domain it holds a lease on, and X must suit all of them. A session marked unattended with no lease at all may write anywhere, so it reads only what every domain's readers may read: in practice the domains everyone signed in may read, and no local domain while any domain is backed up to GitHub. An unattended session is never routed to a connected server either, and its onboarding (the routing block of `initialize`, `server/discover` and the `onboarding` prompt) names only the domains it may read. An interactive session reads as before.

It also writes only into the domains it works on: the domain it was started for, or each domain it holds a lease on when it names none. A run on a private domain reads that domain, so writing into a shared domain could carry what it read to more people. Every write counts: engrams (write, edit, move, split, delete, attachments), `write_reflection`, `discuss`, `share_changes` and a domain's settings. A write elsewhere is refused with "this run works on harbor and cannot write to eng". A session marked unattended with no lease may write wherever its rights allow: it reads only what every domain's readers may read, so nothing it writes reaches anyone new.

When a run holds a lease without an account, the owner's interactive sessions on that machine read and write like the run, and are never asked to confirm anything. Every tool answer then carries one line that says why, for example "a reflect run on eng is going; until it ends this session reads and writes like that run".

To mark custom automation, for example a cron job that runs `claude -p`, give it its own MCP registration whose command is `crystalline mcp --unattended --domain <domain>`, or, over HTTP, add the header `X-Crystalline-Run-Domain: <domain>` and take the loop's lease with the run's account. An unmarked session that holds no lease is treated as a person's.

#### Sharing a source on purpose

Personal knowledge may belong on a shared page, when a person who may read it decides so. So a person can share a source that the rule refuses, on purpose. This confirmation is a deliberate step for a person, not a security boundary against their own agent: an agent with a shell on the owner's machine can do anything the owner can, and a client that answers questions on its own defeats it. What protects against a confused deputy is the read limit above.

- Over MCP, `write_reflection` asks the person before a write that the rule alone would refuse, unless that person already shared that source domain on this page and the sharing still holds. A client on the 2026-07-28 revision that can put a question to its user gets `input_required`; an older client that declared elicitation gets an elicitation request on the same call. The question names who reads the page in plain words ("everyone who can open this instance, visitors without an account included", "the 3 accounts that may read eng", "the people who can read the GitHub repository acme/kb, where this page is backed up"), shows the text to share, says that it becomes readable by all of them and that the source's name stays hidden from readers who may not read it. Its one box starts unchecked: only a ticked box and Accept share the text. A decline, an unticked box, no answer within ten minutes, or an answer to a question that was never asked (the answer must carry the state of the question it answers, for the same call, once) refuse the write as before. A yes is bound to the readers and the repository the question named: when one of them changed before the write lands, the write is refused with "who may read this page, or where it is backed up, changed since the person was asked; ask them again".
- A client that cannot ask its user is refused as before, and the refusal adds that a person who may read that domain can share it on purpose from a client that can ask them.
- In Fluid, a `source_too_wide` item in the Review has a Share on purpose button. It shows the same question with the section's text, and Share it confirms the source domain on this page for the page's readers of this moment. When the readers changed since the Review was read, the confirmation is refused, the Review is read again, and the item asks the new question. This is also how a person confirms again after the page's readers grew.
- Nobody is ever asked in an unattended session, for the anonymous caller and the open tier, and for a write that a connected machine routes to the server that holds the domain. An agent account with an MCP token is treated like any account: it may confirm only when its client can ask the person at it, and the confirmation is recorded under the account.
- Only a caller who may curate the page and may read the source's engrams can confirm. A source in a domain the caller may not open is refused as an unknown domain, before anything is asked. When nobody here knows who reads which domain (the accounts store could not be opened), nothing is asked and nothing can be confirmed.

One confirmation counts once per person, page and source domain. That person's later edits of the page that draw on the same domain need no new question, whatever the text; another person who cites that domain on the same page confirms for themselves, and a different source domain is asked about on its own. The confirmation is kept on the server, in the index beside the page, never in the page file: who confirmed (the account, or `owner` for the machine owner), when, for which page and source domain, who could read the page then, and the repository the page was backed up to then. A confirmation written into a page file by hand, pulled in from a repository or sent along with a page save has no effect. `read_reflection` shows curators a section's `disclosed` (`by`, `at`), and Fluid's list of a section's sources says "Shared on purpose by keeper on 2026-10-28". A section citing a confirmed source domain is no `source_too_wide` item and is backed up like any other page. It becomes one again when the page's readers widen or its backup target changes: someone reads the page now who did not then and may not read the source (a new member, a private domain made shared, the page's domain connected to another repository), because the person agreed to the readers of that moment. When the source's domain narrows later, nothing changes: its text reached those people already. A write that leaves the page citing that domain nowhere drops its confirmations. A page move carries its confirmations and a page delete drops them. A `reindex --wipe` drops them too, and the queue then names those sections until a person confirms again. Two GitHub repositories are still not compared, and a repository made public is not seen, so a confirmation names the repository and is held against it only.

## Threads

A thread is a discussion about one passage. It is anchored on a section and, if you like, on an exact quoted passage in it. A thread can be opened, answered, resolved with a short outcome and reopened. The threads of a page are stored in `<name>.threads.md` beside the page. The history of a thread is only ever added to: a resolved thread does not change, and a reopen is a new entry.

Answer a person's thread and leave it open. The person who opened it resolves it. An agent that tries to resolve a person's thread right after its own answer is refused. Putting the answer into the resolve outcome is refused too. It may resolve it once the person wrote back, for example "thanks, you can close it". An agent may resolve a thread an agent opened. In Fluid a person's thread with an agent's answer as its last entry shows "Answered, waiting for you" to the person who opened it. A thread an agent opened never shows it.

Agents write each comment as a conventional comment: a first line `<label> [decorations]: <subject>`, for example `question (non-blocking): is this still true after the migration?`. An agent never puts engram links into a thread, because some readers cannot see engrams.

### Labels on comments

A label says what a comment wants. Crystalline follows [conventional comments](https://conventionalcomments.org): the first line of a comment is `<label> (<decorations>): <subject>`, and the lines after it are the discussion. The labels are praise, nitpick, suggestion, issue, todo, question, thought, chore and note. The decorations are blocking, non-blocking and if-minor. Blocking and non-blocking exclude each other, and if-minor goes with either. Other decorations, for example `(security)`, are kept and shown as they are.

For people a label is optional. In Fluid the comment box and the answer box have a Label menu beside the send button. Pick a label there, and then the decorations. The choice shows as a chip above the box, and the box holds only your words. Its first line becomes the subject. You can also type the label at the very start, for example `issue (blocking): `, and it turns into the chip. Pasting a whole conventional comment does the same. The remove button on the chip takes the label off again. A label you type while a chip is set, or after you took the chip off, stays text. A resolve outcome has no label.

Only the exact form is a label. An unknown label, a missing `: ` or an empty subject leave the comment as plain text, and old comments stay as they are. A comment is stored exactly as it was sent, so the label is part of its text.

A thread draws a labelled comment with its label, its decorations and its subject in bold. The thread card shows the icon of the opening comment's label beside the passage. Only blocking is red. When a page has open blocking threads, a Blocking only switch next to Show resolved lists just those.

Agents keep writing every comment as a conventional comment. The `reflect` queue names the label of each thread's opening comment and whether it blocks, and lists a blocking thread before the other threads of its section. Agents answer blocking threads first.

## Who may do what

Access has two axes. They are set side by side: per account for every shared domain, and per member on a private domain.

- The knowledge level decides what someone may do with the engrams: `none` (sees no engram), `viewer`, `editor`, and on a private domain `manager`, on the instance `admin`.
- The Reflection level decides what someone may do with the Reflection: `reader` reads and comments, `contributor` also edits. It only raises what the knowledge level already gives. Without one, the knowledge level decides.

| Knowledge level | Reflection, no level set | with `reader` | with `contributor` |
|---|---|---|---|
| `admin`, `manager`, `editor`, the domain owner, the machine owner | curate | curate | curate |
| `viewer` | read and comment | read and comment | edit |
| `none` | nothing | read and comment | edit |
| anonymous visitor, public domain | read, never comment | - | - |

- **Read and comment** means reading every page, its threads and its files, and taking part in threads.
- **Edit** adds writing pages: create, change, move and delete them, and add new files.
- **Curate** adds writing `PACT.md`, recording the engrams a section was built from, settling feedback, and replacing, moving and removing files. Everyone who may edit the engrams curates. Readers and contributors propose a pact change in a thread.
- A member of a private domain at knowledge `none` needs a Reflection level; without one the membership grants nothing and is refused.
- An account whose write list leaves a domain out at most reads and comments there, whatever its levels say. The list never grants access.
- An anonymous visitor reads a public domain's Reflection and never comments.
- A source of another domain is shown only to a reader who may read engrams in that domain. Anyone else sees a count, for example "2 sources in a domain you cannot open", with no name and no state. The `reflection:` block they get leaves those sources out, and a write by them keeps those sources as they were. An agent that may not open the domain gets the queue item with the words "a source in a domain you cannot open has changed", so it asks a person instead of updating blind. Taking feedback into the engrams of another domain needs the right to write there. Without it, the agent answers in a thread or settles the item as kept.
- A version before 0.26.0 does not know the knowledge level `none` and reads it as `viewer`. So before you go back to an older version, give those accounts and members a level that is right for the older version.

The commands:

```bash
# an account that works on the Reflection of every shared domain without seeing engrams
crystalline users add ada --role none --reflection contributor
# change an account's Reflection level: default, reader or contributor
crystalline users reflection ada reader
# a member of a private domain who reads its Reflection only
crystalline domain members eng add ada --level none --reflection reader
```

## The reflect queue

`reflect` hands an agent the Reflection's work queue. It is computed fresh on every call and changes nothing. There are three kinds of item.

| Kind | `what` | Meaning |
|---|---|---|
| feedback | `new`, `changed`, `removed` | A person added, changed or removed a section since the last reflect. The item carries the diff, who changed it and the engrams the section was built from. |
| update | `source_changed` | An engram a section was built from changed. |
| update | `source_retired` | An engram a section was built from was retired. |
| update | `source_missing` | An engram a section was built from is gone. |
| update | `source_unavailable` | Sources of a page in a domain that cannot be checked here: the domain is not on this instance, or it is a domain of a connected server. One item per page and domain. Keep them while the domain may come back, or leave one out of its section's sources to drop it. |
| update | `source_too_wide` | A section cites a domain fewer people may read than the page's domain. Remove that source and the text drawn from it; the page stays as it is until then, and the backup holds it back. Or a person shares it on purpose: the item carries `question`, what that person is asked. Only a caller who may open that domain gets it. See [Sources from other domains](#sources-from-other-domains) and [Sharing a source on purpose](#sharing-a-source-on-purpose). |
| update | `unreflected` | An engram no section covers yet. |
| update | `off_structure` | A page fits none of the pact's `## Structure` globs. |
| update | `pact_changed` | A person changed the pact since the pages were last brought in line with it. `sections` lists the ids of the changed parts. |
| update | `asset_missing` | A page links a file in `reflections/assets/` that is not there. |
| update | `engram_attachment` | A page embeds an engram attachment instead of a file of its own. |
| update | `asset_unreferenced` | A file in `reflections/assets/` that no page links. |
| update | `host_moved` | The domain moved to this instance on a shared database, and this disk lacks files the previous host wrote. One item per domain, with the counts in `only_elsewhere`. See [One home, one backup](#one-home-one-backup). |
| review | `thread` | A thread waits for the agent. |

Items about the same section stay together. Groups come oldest first, and inside a group feedback comes before update and update before review. So the agent reads a person's change before it rewrites the section.

The agent settles a feedback item in one of three ways:

- It takes the change into the engrams: it changes the engrams, then writes the section with its source engrams.
- It keeps the change as the person's text (`write_reflection` with `action: settle` and `outcome: kept`). A settle without `sections` settles the whole page, its top included. A heading with no id line yet is left out. It gets one on the next write through Crystalline, and a settle after that settles it. When the pages follow a changed pact again, the agent settles `PACT.md` this way.
- It answers in a thread (`discuss` with `answers_feedback`), when it disagrees or has a question.

Three nudges point the agent at open items of the Reflections its user curates: the receipt of a write, the reminder at the end of a session (the Stop hook) and the line for the domain in the routing block at session start ("Reflection: 3 open items - call reflect"). The daemon counts the open items every five minutes.

On a machine connected to a server with `crystalline connect`, the server's domains appear under their names on this machine. The agent works their Reflections through the same four tools, with its user's rights on that server. A queue over all domains asks every server. The nudges for these domains come from the server's counts, which the daemon fetches every three minutes.

## Searching a Reflection

`search_engrams` leaves Reflection pages out. To search them, call `read_reflection` with `query`. The search finds sections, not whole pages.

- `search_type` is `hybrid` (the default), `text` or `semantic`. Hybrid and semantic search answer as text search when there is no embedding model, when no page of this domain is embedded by the active model yet (another domain's vectors do not count), or when this domain's stored vectors belong to another model. The answer's `mode` says which search ran.
- `limit` is 10 by default and at most 100. `page` pages through the hits.
- `path` and `query` together are refused: pass `path` to read one page, or `query` to search them all.
- Each hit names the page, its title, the section id and heading, and a short snippet. A hit never carries a score.

The JSON API searches the same way: `GET /api/v1/domains/{domain}/reflection/search` takes `q`, `search_type`, `limit` (20 by default, at most 100) and `page`.

The sections of each page are cut into chunks and embedded beside the engrams, in a table of their own. A write wakes the embedding worker, so a new page can be found by meaning soon after. `crystalline status` and `crystalline doctor` count the reflection chunks while there are any. When the index is upgraded, every page is read and chunked again once, on the first start; `crystalline reindex --full` does the same.

## Files in a Reflection

A page can show images and other files. They live in `reflections/assets/`, for example `reflections/assets/diagrams/flow.png`. The rules for what may be stored are the ones for engram attachments: the same file types and the same size limit.

A page links a file relative to its own folder. A page in `reflections/` writes `![Flow](assets/diagrams/flow.png)`, and a page in `reflections/guides/` writes `![Flow](../assets/diagrams/flow.png)`. A link may carry the same formatting fragment as an image on an engram, for example `#right,w=50%`.

A page may not embed an engram attachment from the domain's own `assets/` folder. Copy the file into `reflections/assets/` first and link the copy. A write through Crystalline that embeds an engram attachment is refused, and `reflect` reports one that came in another way.

The Reflection's rights decide, and the knowledge rights play no part:

- A reader reads every file.
- A contributor adds a new file.
- A curator replaces, moves and removes files. A move rewrites every link to the file. A move keeps the file's extension, and a change of letter case alone (`a.png` to `A.png`) is a plain rename. A change only in Unicode normal form may be refused as a move onto an existing file.

These files are not engram attachments. The engram sweep never reports them as orphans. `reflect` reports a file no page links and a link to a file that is missing. The backup branch and the domain export carry them.

## One home, one backup

A local domain keeps its Reflection where it is, in its folder.

A domain shared through GitHub has one home: the instance whose `service.public_url` is the domain's `reflection_home`. This key is filled in when Reflection is switched on at that instance, and it is never set by hand. Every other instance answers `this domain's Reflection lives at <url>`. People and agents work on the Reflection at the home.

A plain clone of the team's repository, added as a local domain, is not connected and has no home. Its MANIFEST says `team: github` (the first share from a connected instance writes it), so Crystalline refuses to switch Reflection on there, and a Reflection that is already on in such a clone does not run. Connect the domain (`crystalline domain add <name> --origin <owner>/<repo>`) and switch Reflection on at the team server. A local domain without that line, for example one you back up to your own GitHub repository, keeps its Reflection as before. See [Keep a team repository's Reflection safe](teams.md#keep-a-team-repositorys-reflection-safe).

Several instances can share one Postgres database (see [Shared database collaboration](deployment.md#shared-database-collaboration)). Then one instance serves a domain's Reflection: the one that holds the domain's host lock, which also syncs the domain's files. Every other instance answers `this domain's Reflection is served by <label>; move it with crystalline serve --take-over`, and its status says `hosted_elsewhere` and names the host: the host of its `service.public_url`, else its machine's hostname. `crystalline serve --take-over` on another instance moves the lock and the Reflection there. The new host serves at once, and the old one refuses from its next request. When the host shuts down cleanly, the next instance that is asked for the Reflection takes the lock and serves it. When the host crashed or went to sleep, the next write to the Reflection takes the lock once the old host's last heartbeat is 90 seconds old; a read only names the old host and says when it was last seen. A person who is not signed in sees `another instance` instead of the host's name. A command that runs without a daemon never takes the lock for that: it answers that the Reflection is not served by a command and that you start the daemon. It serves the Reflection only while this machine already holds the lock, through its daemon, a `crystalline mcp --embedded` session or another command. For a domain shared through GitHub, the host also needs to be the `reflection_home`.

Each instance keeps the Reflection's files on its own disk: the pages, the threads, the hidden baselines and the files under `reflections/assets/`. The files on disk are the truth, so when the Reflection moves to another instance, copy the domain folder to the new host, or keep it on a volume both instances share. For a domain shared through GitHub, the Reflection can also come back from the `crystalline/reflections` branch (see [Restore from the backup](#restore-from-the-backup)). A new host never writes a page from the database to its disk, and never deletes the row of a page it does not have. It keeps those rows, its queue shows one `host_moved` item with the counts, and it does not back up the Reflection. While the item is open, a write that would create one of those pages, files or engrams on this disk is refused with the same remedy (409 over the JSON API): a whole-page write, a page move onto its path, an upload to its path, a file move onto it, an overwriting capture of an engram and an engram move onto its path. An edit or a delete of such a file or engram is refused too, because its row is the only shared copy. A row this instance wrote after the move, whose file was then deleted by hand, is refused the same way until the move is resolved. Pages without their baseline raise no feedback while the item is open. The item goes away when a sync finds every file, or when a person runs `crystalline sync --domain <name> --accept-disk` after the item appeared, which deletes the rows of the files that are still missing.

The home backs the whole `reflections/` folder up to the branch `crystalline/reflections` of the domain's repository, with its own GitHub token:

- At once after an agent's write, a comment, a settle, a move or a delete, and when a co-editing session closes.
- After one minute without a new edit, for a person's edits in a co-editing session or a change on disk.

The branch starts empty and never carries an engram. It keeps each page as it is on disk, so it also holds the names of other domains a page cites as sources, and the team's repository shows them. The domain archive an admin downloads carries the pages the same way. "Share changes" never offers a file under `reflections/`. A change marked while the daemon restarts is committed with the next one. A failed backup is tried again later, with a growing wait up to one hour.

To move the home to a new `service.public_url` or to another server, copy the `reflections/` folder there first (the restore steps below work for that too). Then switch Reflection off and on again at the new home, in one go. That fills in `reflection_home` again. Do not share changes between the two steps: while the switch is off, "share changes" offers the files in `reflections/` as engram changes. Share the MANIFEST change at once after the switch-on. Until the old home pulls it, both instances act as the home and back up to the same branch. The old home stops when it pulls the change.

## Restore from the backup

1. Stop the daemon.
2. Download the branch: `git clone --branch crystalline/reflections --single-branch <repo> /tmp/reflection-backup`, or use the forge's "Download ZIP" for that branch.
3. Copy its `reflections/` folder over the domain's `reflections/` folder. The copy includes the hidden folder `.reflected/` (the text at the last reflect), the threads and `PACT.md`.
4. Start the daemon.
5. Run `crystalline reindex` to bring the reflection index back.

The next backup commits whatever differs from the head of the branch. Files deleted locally while the backup state was lost may stay on the branch until such a restore.

A domain export (the archive download in Fluid or `crystalline domain export`) also carries the Reflection while the switch is on: its pages, threads, files and the hidden baseline in `.reflected/`. An import leaves `reflections/` out where a Reflection keeps that folder, because its pages are not engrams. Restore a Reflection from its backup branch as above.

## A scheduled run

`crystalline reflect run` works the queue with a headless agent: it checks whether anything is due, takes the lease, runs Claude Code with the shipped prompt, shares what changed in the engrams as a pull request, records the run and releases the lease. With nothing due it ends at once and starts no agent. `claude` must be on the `PATH` of the run.

On a team server, run it in remote mode against the home (`CRYSTALLINE_REMOTE_URL` and `CRYSTALLINE_REMOTE_TOKEN`) with the `cmt_` token of an agent account that may curate the Reflection. A run in local mode acts as the machine owner, so it cannot tell its own writes from the edits people make in Fluid during the run, and the daemon's socket under the service user is out of reach of other accounts anyway. With systemd, use two files. `/etc/systemd/system/crystalline-reflect.service`:

```ini
[Unit]
Description=Work the Reflection queue of eng

[Service]
Type=oneshot
DynamicUser=yes
StateDirectory=crystalline-reflect
Environment=HOME=%S/crystalline-reflect
Environment=PATH=/usr/local/bin:/usr/bin:/bin
Environment=CRYSTALLINE_REMOTE_URL=https://crystalline.example.com
LoadCredential=remote-token:/etc/crystalline/reflect-runner-token
Environment=CRYSTALLINE_REMOTE_TOKEN_FILE=%d/remote-token
# Holds ANTHROPIC_API_KEY=... (or CLAUDE_CODE_OAUTH_TOKEN=...); keep it readable by root only
EnvironmentFile=/etc/crystalline/reflect-runner.env
ExecStart=/usr/local/bin/crystalline reflect run --domain eng --max-items 20 --runner timer
SuccessExitStatus=1
```

`SuccessExitStatus=1` because `run` exits 1 when nothing is due. `/etc/systemd/system/crystalline-reflect.timer`:

```ini
[Unit]
Description=Work the Reflection queue of eng every day

[Timer]
OnCalendar=*-*-* 06:30:00
Persistent=true

[Install]
WantedBy=timers.target
```

Enable the timer with `systemctl enable --now crystalline-reflect.timer`.

On a machine only you use, a run against your own daemon is fine. Cron, daily at 06:30, with the folder Claude Code is installed in on the `PATH`:

```bash
PATH=/usr/local/bin:/usr/bin:/bin:/home/you/.local/bin
30 6 * * * crystalline reflect run --domain eng --max-items 20 >> /home/you/crystalline-reflect.log 2>&1
```

For answers within a minute, `crystalline reflect watch` runs as soon as something is due. A headless run asks people in a thread and leaves what needs a yes for a person; the Reflection home shows "Last reflected" and how many items wait for a person. [Run the loops on their own](automation.md) has the details, the n8n, Kubernetes and GitHub Actions examples and the webhook that starts a runner when work is due. `crystalline install` does not set up the scheduled run.
