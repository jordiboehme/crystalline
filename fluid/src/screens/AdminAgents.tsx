/**
 * Administration > Agents: the identities scheduled reflect and evolve runs
 * on another machine use, each with its own tokens.
 *
 * An agent is made with its first token. A token is shown once, in a field
 * with a copy button, and lives only in this screen's state: Done or leaving
 * the page takes it away for good. An agent holds at most two live tokens,
 * so a new one can be issued before the old one is revoked; the server
 * refuses a third in its own sentence.
 *
 * Agents work only with `auth.mcp` on. The list says whether it is, from the
 * same setting the server checks a new agent against, so the page says so at
 * the top and holds the create button back with that reason.
 *
 * Runs are not shown here. Run records live per domain, so the last run of an
 * agent would cost one request per domain; a domain's automation card shows
 * them.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";

import {
  AGENTS_KEY,
  createAgent,
  fetchAgents,
  issueAgentToken,
  removeAgent,
  revokeAgentToken,
} from "../api/agents";
import { problemDetail } from "../api/client";
import type { Agent, AgentLoops, AgentToken } from "../api/model";
import { useAuth } from "../auth/AuthContext";
import {
  DestructiveAction,
  READ_ONLY_REASON,
} from "../components/DestructiveAction";
import { BUTTON, CONTROL_HEIGHT, FIELD } from "../components/primitives";
import { formatInstant } from "../format";
import { refreshAfterWrite } from "../query/refreshAfterWrite";
import { usePageTitle } from "../usePageTitle";
import type { Notice } from "./accounts";
import { Labelled, NoticeLine, PageHeader, ShownOnce } from "./AdminParts";

/** What the create button says while `auth.mcp` is off. */
const NEEDS_AUTH_MCP =
  "Agents work only with auth.mcp on, and it is off on this instance.";

/** A live token that ends within this many days is marked. */
const WARN_DAYS = 14;

const DAY_MS = 24 * 60 * 60 * 1000;

const LOOPS: readonly { value: AgentLoops; label: string }[] = [
  { value: "both", label: "Reflect and evolve" },
  { value: "reflect", label: "Reflect" },
  { value: "evolve", label: "Evolve" },
];

const PRIMARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.primary} aria-disabled:cursor-default aria-disabled:opacity-50`;
const SECONDARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50`;
const HINT = "text-xs text-slate-500 dark:text-slate-400";

/** A token shown once, and which agent it belongs to. */
interface Revealed {
  agent: string;
  token: string;
}

export default function AdminAgents() {
  usePageTitle("Agents");
  const queryClient = useQueryClient();
  const { capabilities } = useAuth();
  const [notice, setNotice] = useState<Notice | null>(null);
  const [revealed, setRevealed] = useState<Revealed | null>(null);

  const agents = useQuery({ queryKey: AGENTS_KEY, queryFn: fetchAgents });
  // Unknown until the list answers, and held back until then, so a fast
  // press cannot meet the server's refusal the page was about to show.
  const authMcp = agents.data?.auth_mcp ?? false;
  const createReason = capabilities.readOnly
    ? READ_ONLY_REASON
    : authMcp || agents.data === undefined
      ? undefined
      : NEEDS_AUTH_MCP;

  const refresh = () =>
    refreshAfterWrite(queryClient, { queryKey: AGENTS_KEY });

  function problem(error: Error) {
    setNotice({ kind: "problem", text: problemDetail(error) });
  }

  const rows = agents.data?.agents ?? [];

  return (
    <div className="flex max-w-4xl flex-col gap-6">
      <PageHeader title="Agents">
        Identities for scheduled reflect and evolve runs on another machine.
        Each agent has its own tokens.
      </PageHeader>

      {agents.data !== undefined && !agents.data.auth_mcp && (
        <div
          role="note"
          aria-label="auth.mcp is off"
          className="flex flex-col gap-1 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
        >
          <p>{NEEDS_AUTH_MCP} Until it is on, no agent token opens anything.</p>
          <p>
            Turn it on with{" "}
            <code className="font-mono">
              crystalline config set auth.mcp true
            </code>
            . After a change, restart the server.
          </p>
        </div>
      )}

      {notice && <NoticeLine kind={notice.kind} text={notice.text} />}

      {revealed && (
        <ShownOnce
          key={revealed.token}
          label={`Token for ${revealed.agent}`}
          copyLabel="Copy the token"
          value={revealed.token}
          note={
            <p className="text-sm">
              The runner sends it as{" "}
              <code className="font-mono">Authorization: Bearer</code>, for
              example in{" "}
              <code className="font-mono">CRYSTALLINE_REMOTE_TOKEN</code>.
            </p>
          }
          onDone={() => {
            setRevealed(null);
          }}
        />
      )}

      <CreateAgent
        waiting={agents.data === undefined}
        disabledReason={createReason}
        onCreated={(name, token) => {
          setNotice({ kind: "done", text: `Added ${name}.` });
          setRevealed({ agent: name, token });
          void refresh();
        }}
        onProblem={problem}
      />

      <section aria-labelledby="agents-list" className="flex flex-col gap-4">
        <h2 id="agents-list" className="text-section">
          Agents on this instance
        </h2>
        {agents.error && (
          <NoticeLine kind="problem" text={problemDetail(agents.error)} />
        )}
        {agents.isPending ? (
          <p className="text-sm text-muted">Loading...</p>
        ) : rows.length === 0 && !agents.error ? (
          <p className="text-sm text-muted">No agents yet.</p>
        ) : (
          <ul className="flex flex-col gap-4">
            {rows.map((agent) => (
              <li key={agent.name}>
                <AgentCard
                  agent={agent}
                  readOnly={capabilities.readOnly}
                  onIssued={(token) => {
                    setNotice({
                      kind: "done",
                      text: `Issued a token for ${agent.name}.`,
                    });
                    setRevealed({ agent: agent.name, token });
                    void refresh();
                  }}
                  onRevoked={(label) => {
                    setNotice({
                      kind: "done",
                      text: `Revoked ${label} of ${agent.name}.`,
                    });
                    void refresh();
                  }}
                  onRemoved={() => {
                    setNotice({ kind: "done", text: `Removed ${agent.name}.` });
                    if (revealed?.agent === agent.name) setRevealed(null);
                    void refresh();
                  }}
                  onProblem={problem}
                />
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

/** The form that makes an agent and its first token. */
function CreateAgent({
  waiting,
  disabledReason,
  onCreated,
  onProblem,
}: {
  /** The list has not answered yet, so whether agents work is unknown. */
  waiting: boolean;
  disabledReason: string | undefined;
  onCreated: (name: string, token: string) => void;
  onProblem: (error: Error) => void;
}) {
  const ids = useId();
  const [name, setName] = useState("");
  const [display, setDisplay] = useState("");
  const [loops, setLoops] = useState<AgentLoops>("both");
  const [label, setLabel] = useState("");
  const [days, setDays] = useState("90");
  const create = useMutation({
    // Never retried: a POST that committed and then lost its answer would
    // make a second token nobody ever sees.
    retry: false,
    // The token goes to the caller and is not returned, so it never sits in
    // the mutation cache.
    mutationFn: async () => {
      const created = await createAgent({
        name: name.trim(),
        display: display.trim() === "" ? null : display.trim(),
        loops,
        label: label.trim() === "" ? null : label.trim(),
        expires_days: Number(days),
      });
      onCreated(created.agent.name, created.token.token);
      return created.agent.name;
    },
    onSuccess: () => {
      setName("");
      setDisplay("");
      setLoops("both");
      setLabel("");
      setDays("90");
    },
    onError: onProblem,
  });
  const disabled = waiting || disabledReason !== undefined || create.isPending;
  return (
    <section
      aria-labelledby={`${ids}-title`}
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id={`${ids}-title`} className="text-section">
        Add an agent
      </h2>
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (disabled || name.trim() === "") return;
          create.mutate();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-3">
          <Labelled label="Name">
            {(id) => (
              <input
                id={id}
                required
                autoComplete="off"
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </Labelled>
          <Labelled label="Display name">
            {(id) => (
              <input
                id={id}
                autoComplete="off"
                value={display}
                placeholder="Same as the name"
                onChange={(event) => {
                  setDisplay(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </Labelled>
          <Labelled label="Loops">
            {(id) => (
              <select
                id={id}
                value={loops}
                onChange={(event) => {
                  setLoops(event.target.value as AgentLoops);
                }}
                className={FIELD}
              >
                {LOOPS.map((loop) => (
                  <option key={loop.value} value={loop.value}>
                    {loop.label}
                  </option>
                ))}
              </select>
            )}
          </Labelled>
          <Labelled label="First token label">
            {(id) => (
              <input
                id={id}
                autoComplete="off"
                value={label}
                placeholder="The day it is issued"
                onChange={(event) => {
                  setLabel(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </Labelled>
          <Labelled label="Token life in days">
            {(id) => (
              <input
                id={id}
                type="number"
                min={1}
                max={365}
                required
                value={days}
                onChange={(event) => {
                  setDays(event.target.value);
                }}
                className={FIELD}
              />
            )}
          </Labelled>
        </div>
        <p className={HINT}>
          The name goes into commits and run records. A token lives 1 to 365
          days.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="submit"
            aria-disabled={disabled}
            aria-describedby={
              disabledReason !== undefined ? `${ids}-reason` : undefined
            }
            className={PRIMARY}
          >
            Add agent
          </button>
          {disabledReason !== undefined && (
            <span id={`${ids}-reason`} className={HINT}>
              {disabledReason}
            </span>
          )}
        </div>
      </form>
    </section>
  );
}

/** Milliseconds from now until `iso`; null when it does not parse. */
function timeLeft(iso: string, now: number): number | null {
  const at = new Date(iso).getTime();
  if (Number.isNaN(at)) return null;
  return at - now;
}

/** How a loops value reads. */
function loopsWords(loops: AgentLoops): string {
  return LOOPS.find((loop) => loop.value === loops)?.label ?? loops;
}

/** One agent: who it is, its tokens, a new token and its removal. */
function AgentCard({
  agent,
  readOnly,
  onIssued,
  onRevoked,
  onRemoved,
  onProblem,
}: {
  agent: Agent;
  readOnly: boolean;
  onIssued: (token: string) => void;
  onRevoked: (label: string) => void;
  onRemoved: () => void;
  onProblem: (error: Error) => void;
}) {
  const ids = useId();
  const [label, setLabel] = useState("");
  const [days, setDays] = useState("90");
  const issue = useMutation({
    retry: false,
    mutationFn: async () => {
      const issued = await issueAgentToken(agent.name, {
        label: label.trim() === "" ? null : label.trim(),
        expires_days: Number(days),
      });
      onIssued(issued.token);
      return issued.id;
    },
    onSuccess: () => {
      setLabel("");
      setDays("90");
    },
    onError: onProblem,
  });
  const revoke = useMutation({
    mutationFn: (token: AgentToken) =>
      revokeAgentToken(agent.name, token.id).then(() => token.label),
    onSuccess: onRevoked,
    onError: onProblem,
  });
  const remove = useMutation({
    mutationFn: () => removeAgent(agent.name),
    onSuccess: onRemoved,
    onError: onProblem,
  });
  // Read once when the card mounts: a day is the unit, so a page left open
  // for hours marks a token a little late at worst.
  const [now] = useState(() => Date.now());
  const issueDisabled = readOnly || issue.isPending;
  return (
    <section
      aria-labelledby={`${ids}-name`}
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 id={`${ids}-name`} className="text-base font-semibold break-words">
          {agent.display}
        </h3>
        <code className="font-mono text-sm break-all text-ink-3">
          {agent.name}
        </code>
        <span className="text-sm text-ink-3">
          Loops: {loopsWords(agent.loops)}
        </span>
        {agent.disabled && (
          <span className="text-sm text-red-700 dark:text-red-300">
            Disabled
          </span>
        )}
      </header>

      {agent.tokens.length === 0 ? (
        <p className="text-sm text-muted">No tokens.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Tokens of {agent.name}</caption>
            <thead className={HINT}>
              <tr>
                <th scope="col" className="py-1 pr-3 font-normal">
                  Label
                </th>
                <th scope="col" className="py-1 pr-3 font-normal">
                  State
                </th>
                <th scope="col" className="py-1 pr-3 font-normal">
                  Expires
                </th>
                <th scope="col" className="py-1 pr-3 font-normal">
                  Last used
                </th>
                <th scope="col" className="py-1 font-normal">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {agent.tokens.map((token) => {
                const left = timeLeft(token.expires_at, now);
                // The exact time decides, so 13.2 days left is marked; the
                // sentence rounds up to whole days.
                const soon =
                  token.state === "live" &&
                  left !== null &&
                  left < WARN_DAYS * DAY_MS;
                const days = left === null ? 0 : Math.ceil(left / DAY_MS);
                return (
                  <tr
                    key={token.id}
                    className="border-t border-slate-200 align-top dark:border-slate-800"
                  >
                    <td className="py-2 pr-3 break-words">{token.label}</td>
                    <td className="py-2 pr-3">{stateWords(token.state)}</td>
                    <td className="py-2 pr-3">
                      {formatInstant(token.expires_at)}
                      {soon && (
                        <span className="mt-0.5 block text-xs text-amber-800 dark:text-amber-200">
                          {days <= 1
                            ? "Ends within a day."
                            : `Ends in ${days} days.`}
                        </span>
                      )}
                    </td>
                    <td className="py-2 pr-3">
                      {token.last_used
                        ? formatInstant(token.last_used)
                        : "Never"}
                    </td>
                    <td className="py-2">
                      {token.state === "live" && (
                        <DestructiveAction
                          label="Revoke"
                          ariaLabel={`Revoke ${token.label} (token ${token.id})`}
                          confirmLabel="Confirm revoke"
                          confirmAriaLabel={`Confirm revoke of token ${token.id}`}
                          pending={
                            revoke.isPending && revoke.variables.id === token.id
                          }
                          disabledReason={
                            readOnly ? READ_ONLY_REASON : undefined
                          }
                          onConfirm={() => {
                            revoke.mutate(token);
                          }}
                        />
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <form
        aria-label={`New token for ${agent.name}`}
        className="flex flex-wrap items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (issueDisabled) return;
          issue.mutate();
        }}
      >
        <Labelled label="Token label">
          {(id) => (
            <input
              id={id}
              autoComplete="off"
              value={label}
              placeholder="The day it is issued"
              onChange={(event) => {
                setLabel(event.target.value);
              }}
              className={FIELD}
            />
          )}
        </Labelled>
        <Labelled label="Life in days">
          {(id) => (
            <input
              id={id}
              type="number"
              min={1}
              max={365}
              required
              value={days}
              onChange={(event) => {
                setDays(event.target.value);
              }}
              className={`w-24 ${FIELD}`}
            />
          )}
        </Labelled>
        <button
          type="submit"
          aria-label={`Issue a token for ${agent.name}`}
          aria-disabled={issueDisabled}
          className={SECONDARY}
        >
          Issue token
        </button>
      </form>
      <p className={HINT}>
        Two tokens can be live at once, so a new one can start before the old
        one is revoked.
      </p>

      <div className="flex flex-col gap-1 border-t border-slate-200 pt-3 dark:border-slate-800">
        <p className={HINT}>
          Removing the agent ends its tokens and discards its drafts.
        </p>
        <DestructiveAction
          label="Remove"
          ariaLabel={`Remove ${agent.name}`}
          confirmLabel="Confirm remove"
          confirmAriaLabel={`Confirm remove of ${agent.name}`}
          pending={remove.isPending}
          disabledReason={readOnly ? READ_ONLY_REASON : undefined}
          onConfirm={() => {
            remove.mutate();
          }}
        />
      </div>
    </section>
  );
}

/** How a token's state reads. */
function stateWords(state: AgentToken["state"]): string {
  switch (state) {
    case "live":
      return "Live";
    case "expired":
      return "Expired";
    case "revoked":
      return "Revoked";
    default:
      return state;
  }
}
