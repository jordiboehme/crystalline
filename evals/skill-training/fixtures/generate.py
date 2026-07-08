"""Generate the fixture workspaces for the routing benchmark.

Builds every workspace under fixtures/workspaces/ with the real
crystalline binary, so the fixtures are format-true by construction:
domains are scaffolded with `domain init`, engrams written with
`crystalline write` and the result checked with `crystalline verify`.
Registration state and the index used during generation live in a
throwaway directory; the committed fixture is only the domain folders.

Regenerate after editing the content tables below (name workspaces to
rebuild only those and leave the rest untouched):

    bash fixtures/generate.sh
    bash fixtures/generate.sh aurora
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HARNESS_ROOT = Path(__file__).resolve().parent.parent
WORKSPACES_ROOT = HARNESS_ROOT / "fixtures" / "workspaces"
CRYSTALLINE_BIN = str(
    Path(os.environ.get("CRYSTALLINE_BIN", ""))
    if os.environ.get("CRYSTALLINE_BIN")
    else HARNESS_ROOT.parent.parent / "target" / "release" / "crystalline"
)


def engram(
    title: str,
    content: str,
    *,
    tags: str = "",
    status: str = "",
    engram_type: str = "",
    metadata: dict | None = None,
) -> dict:
    return {
        "title": title,
        "content": content,
        "tags": tags,
        "status": status,
        "type": engram_type,
        "metadata": metadata or {},
    }


WORKSPACES: dict[str, dict] = {
    # Multi-product company: tests narrow targeting and the sweep rule for
    # knowledge that lives in a counter-intuitive domain.
    "acme": {
        "payments": {
            "scope": [
                "Payment processing: retries, refunds and settlement",
                "Billing, invoicing and chargebacks",
            ],
            "when": [
                "Questions about payment flows, retries, refunds, invoices or chargebacks",
            ],
            "notes": ["Escalation rosters live with people-ops, not here"],
            "engrams": [
                engram(
                    "Retry queue architecture",
                    "How failed payment jobs are queued and replayed.\n\n"
                    "- [fact] The retry queue is backed by Redis streams with one consumer group per worker pool #payments\n"
                    "- [fact] Jobs carry an idempotency key so a replay is always safe #payments",
                    tags="architecture,payments",
                    engram_type="architecture",
                ),
                engram(
                    "Retry queue gotcha",
                    "A pitfall that regularly surprises new payment work.\n\n"
                    "- [gotcha] The retry queue drops jobs older than 24 hours #payments\n"
                    "- depends_on [[Retry Queue Architecture]]",
                    tags="gotcha,payments",
                ),
                engram(
                    "Chargeback handling",
                    "The agreed way to fight a chargeback.\n\n"
                    "- [decision] Chargebacks are disputed through the processor dashboard, never by manual database edits #payments\n"
                    "- [fact] Dispute evidence is due within 10 days of the chargeback notice #payments",
                    tags="decision,payments",
                    engram_type="decision",
                ),
                engram(
                    "Invoice numbering convention",
                    "How invoice identifiers are constructed.\n\n"
                    "- [convention] Invoices use the format ACME-YYYY-NNNNN, issued sequentially per calendar year #payments\n"
                    "- [fact] The sequence resets on January 1st each year #payments",
                    tags="convention,payments",
                ),
                engram(
                    "Card vault migration lesson",
                    "What the card vault migration taught us.\n\n"
                    "- [lesson] The card vault migration needed 30 days of dual-write before cutover; the first attempt without it corrupted tokens #payments\n"
                    "- [risk] Skipping dual-write on vault changes risks silent token corruption #payments",
                    tags="lesson,payments",
                ),
                engram(
                    "Refund settlement",
                    "How long customers wait for a refund to land.\n\n"
                    "- [fact] Refunds settle on the customer statement within 5 business days #payments\n"
                    "- [fact] Instant refunds are not offered on any plan #payments",
                    tags="payments",
                ),
            ],
        },
        "infra": {
            "scope": [
                "Deployments and rollbacks",
                "Kubernetes, databases and logging operations",
            ],
            "when": [
                "Questions about deploying, infra incidents, database limits or observability",
            ],
            "notes": ["On-call compensation is a people-ops topic; only the tooling lives here"],
            "engrams": [
                engram(
                    "Deploy pipeline",
                    "The shape and duration of a production deploy.\n\n"
                    "- [fact] Production deploys run blue-green through Buildkite and take about 18 minutes end to end #infra\n"
                    "- [fact] Deploys are cut automatically from main after CI passes #infra",
                    tags="infra,deploy",
                ),
                engram(
                    "Rollback runbook",
                    "What to do when a deploy has to come back out.\n\n"
                    "- [fact] A rollback re-promotes the previous color and completes in about 90 seconds #infra\n"
                    "- depends_on [[Deploy Pipeline]]",
                    tags="infra,runbook",
                    engram_type="runbook",
                ),
                engram(
                    "Postgres connection gotcha",
                    "A database limit that bites under load.\n\n"
                    "- [gotcha] pgbouncer caps client connections at 400 per pool; raising it needs a coordinated restart #infra\n"
                    "- [fact] Connection pool sizing is owned by the platform team #infra",
                    tags="gotcha,infra",
                ),
                engram(
                    "Log retention",
                    "How long logs and aggregates are kept.\n\n"
                    "- [fact] Raw logs are kept for 30 days, aggregated metrics for 13 months #infra\n"
                    "- [fact] Audit logs are exempt and kept for 7 years #infra",
                    tags="infra",
                ),
                engram(
                    "Terraform state convention",
                    "How infrastructure state is laid out.\n\n"
                    "- [convention] One Terraform state per environment, locked through a DynamoDB table #infra\n"
                    "- [convention] Module versions are pinned; no floating references #infra",
                    tags="convention,infra",
                ),
            ],
        },
        "people-ops": {
            "scope": [
                "Hiring, onboarding and interview process",
                "Compensation, expenses and duty rosters",
            ],
            "when": [
                "Questions about hiring, expenses, stipends, rosters or people processes",
            ],
            "notes": [],
            "engrams": [
                engram(
                    "Expense policy",
                    "What needs a receipt and what does not.\n\n"
                    "- [fact] Expenses under 75 EUR need no receipt #people-ops\n"
                    "- [fact] Expenses of 75 EUR or more need an itemized receipt #people-ops",
                    tags="policy,people-ops",
                ),
                engram(
                    "On-call compensation",
                    "How on-call duty is compensated.\n\n"
                    "- [fact] An on-call week pays a 500 EUR stipend, booked through payroll #people-ops\n"
                    "- [fact] The stipend applies regardless of whether pages fire #people-ops",
                    tags="people-ops",
                ),
                engram(
                    "Refund escalation rotation",
                    "Who to page when a refund case escalates.\n\n"
                    "- [fact] Refund escalations page the support duty manager; the rotation lives in the paging tool and rotates weekly #people-ops\n"
                    "- [fact] Weekend escalations page the same rotation, not the infra on-call #people-ops",
                    tags="people-ops,rotation",
                ),
                engram(
                    "Interview loop convention",
                    "How we run interviews.\n\n"
                    "- [convention] The interview loop has four stages and no take-home exercise #people-ops\n"
                    "- [convention] Every loop includes one values interview #people-ops",
                    tags="convention,people-ops",
                ),
                engram(
                    "Onboarding buddy pattern",
                    "How new hires get up to speed.\n\n"
                    "- [pattern] Every new hire gets an onboarding buddy for the first 6 weeks #people-ops\n"
                    "- [pattern] Buddies meet their hire twice a week during the buddy period #people-ops",
                    tags="pattern,people-ops",
                ),
            ],
        },
    },
    # One domain, heavy status variance: tests status filtering for current
    # facts and the no-filter rule for history questions.
    "chronos": {
        "platform": {
            "scope": [
                "Platform engineering: deployment tooling, service auth and data pipelines",
                "Holds the current state and the superseded history side by side",
            ],
            "when": [
                "Questions about how the platform deploys, authenticates or moves data, including how that changed over time",
            ],
            "notes": ["Check status before treating an engram as current fact"],
            "engrams": [
                engram(
                    "Deployment process",
                    "The current deployment mechanism.\n\n"
                    "- [fact] Deploys run through ArgoCD GitOps; a merge to main reconciles automatically #platform\n"
                    "- supersedes [[Deployment Process Jenkins Era]]",
                    tags="platform,deploy",
                ),
                engram(
                    "Deployment process Jenkins era",
                    "The previous deployment mechanism, kept for history.\n\n"
                    "- [fact] Deploys ran through scripted Jenkins pipelines triggered by release tags #platform\n"
                    "- superseded_by [[Deployment Process]]",
                    tags="platform,deploy",
                    status="superseded",
                ),
                engram(
                    "Service auth",
                    "How services authenticate to each other today.\n\n"
                    "- [fact] Services authenticate with short-lived OIDC tokens that expire after 15 minutes #platform\n"
                    "- supersedes [[Service Auth Shared Secrets]]",
                    tags="platform,auth",
                ),
                engram(
                    "Service auth shared secrets",
                    "The retired authentication mechanism.\n\n"
                    "- [fact] Services used static shared secrets; forbidden since the 2025 security audit #platform\n"
                    "- superseded_by [[Service Auth]]",
                    tags="platform,auth",
                    status="deprecated",
                ),
                engram(
                    "Pipeline orchestrator",
                    "The current data pipeline orchestrator.\n\n"
                    "- [fact] Data pipelines are orchestrated by Dagster #platform\n"
                    "- supersedes [[Pipeline Orchestrator Airflow Era]]",
                    tags="platform,data",
                ),
                engram(
                    "Pipeline orchestrator Airflow era",
                    "The previous data pipeline orchestrator, kept for history.\n\n"
                    "- [fact] Data pipelines ran on Airflow with one DAG per team #platform\n"
                    "- superseded_by [[Pipeline Orchestrator]]",
                    tags="platform,data",
                    status="superseded",
                ),
                engram(
                    "Event bus proposal",
                    "A pending proposal, not current fact.\n\n"
                    "- [proposal] Move the event bus to NATS JetStream; sizing and cost are still open #platform\n"
                    "- [idea] A spike against a managed JetStream cluster would settle the throughput question #platform",
                    tags="platform,proposal",
                    status="proposed",
                ),
            ],
        },
    },
    # Bounded validity windows: tests window filters for point-in-time
    # questions and status current for present-day ones.
    "horizon": {
        "policies": {
            "scope": [
                "Commercial and operational policies with explicit validity windows",
                "Pricing, tiers, data residency and dated workarounds",
            ],
            "when": [
                "Questions about pricing or policy, especially what applied at a given date",
            ],
            "notes": ["Several engrams carry valid_from and valid_to; absence means unbounded"],
            "engrams": [
                engram(
                    "Standard plan pricing",
                    "The price of the standard plan since 2026.\n\n"
                    "- [fact] The standard plan costs 49 EUR per month #policies\n"
                    "- supersedes [[Standard Plan Pricing 2025]]",
                    tags="pricing,policies",
                    metadata={"valid_from": "2026-01-01"},
                ),
                engram(
                    "Standard plan pricing 2025",
                    "What the standard plan cost during 2025.\n\n"
                    "- [fact] The standard plan cost 39 EUR per month #policies\n"
                    "- superseded_by [[Standard Plan Pricing]]",
                    tags="pricing,policies",
                    status="superseded",
                    metadata={"valid_from": "2025-01-01", "valid_to": "2025-12-31"},
                ),
                engram(
                    "Free tier policy",
                    "The limits of the free tier.\n\n"
                    "- [fact] The free tier is capped at 3 projects per account #policies\n"
                    "- [fact] Free tier projects pause after 30 days of inactivity #policies",
                    tags="pricing,policies",
                ),
                engram(
                    "Cert rotation workaround",
                    "A temporary operational workaround with a known expiry.\n\n"
                    "- [gotcha] Certificates rotate manually until the automation lands; this workaround expires 2026-09-30 #policies\n"
                    "- [fact] Rotation is due on the first Monday of each month until then #policies",
                    tags="policies,workaround",
                    metadata={"valid_to": "2026-09-30"},
                ),
                engram(
                    "Data residency policy",
                    "Where EU customer data lives.\n\n"
                    "- [fact] EU customer data stays in the eu-central-1 region #policies\n"
                    "- [fact] Backups replicate only within the EU #policies",
                    tags="policies,residency",
                    metadata={"valid_from": "2025-07-01"},
                ),
            ],
        },
    },
    # Schema-governed workspace for the crystalline-schema benchmark:
    # a warn schema with deliberate violations (warnings keep generation
    # green), a strict schema over fully conforming engrams (so a new
    # nonconforming write becomes a verify error), a warn schema ready
    # for strict promotion and an unschema'd corpus for inference.
    "meridian": {
        "delivery": {
            "scope": [
                "Delivery workstream knowledge with schema-governed engram types: decisions, playbooks and retros",
                "Incident notes that have not been given a schema yet",
            ],
            "when": [
                "Questions about delivery decisions, playbooks, retros or incidents and whether they conform to their schemas",
            ],
            "notes": ["Three engram types here carry schemas; check conformance before restructuring"],
            "engrams": [
                engram(
                    "Decision Schema",
                    "The shape for decision engrams in this domain.\n\n"
                    "- [convention] Decisions carry a one line summary and a priority #delivery\n"
                    "- [convention] The deciding owner is recorded in frontmatter #delivery",
                    tags="schema,delivery",
                    engram_type="schema",
                    metadata={
                        "entity": "decision",
                        "version": 1,
                        "schema": {
                            "summary": "string, one line summary of the decision",
                            "rationale?": "string",
                            "priority(enum)": ["low", "medium", "high"],
                            "supersedes?": "Decision",
                        },
                        "settings": {
                            "validation": "warn",
                            "frontmatter": {"owner": "string"},
                        },
                    },
                ),
                engram(
                    "Adopt trunk based development",
                    "A branching model decision.\n\n"
                    "- [summary] All delivery repos move to trunk based development\n"
                    "- [rationale] Long lived branches caused two release trainwrecks in Q1\n"
                    "- [priority] high",
                    tags="delivery,decision",
                    engram_type="decision",
                    metadata={"owner": "dana"},
                ),
                engram(
                    "Choose postgres for ledger",
                    "A storage decision.\n\n"
                    "- [summary] The ledger service stores balances in postgres\n"
                    "- [priority] medium",
                    tags="delivery,decision",
                    engram_type="decision",
                    metadata={"owner": "miguel"},
                ),
                engram(
                    "Retire nightly batch",
                    "A pipeline decision.\n\n"
                    "- [summary] The nightly reconciliation batch is retired in favor of streaming\n"
                    "- [rationale] The batch window kept colliding with the backup window\n"
                    "- [priority] low",
                    tags="delivery,decision",
                    engram_type="decision",
                    metadata={"owner": "dana"},
                ),
                engram(
                    "Standardize error budgets",
                    "A reliability decision.\n\n"
                    "- [summary] Every delivery service gets a quarterly error budget\n"
                    "- [priority] medium",
                    tags="delivery,decision",
                    engram_type="decision",
                    metadata={"owner": "priya"},
                ),
                engram(
                    "Rushed cache migration",
                    "A decision captured in a hurry; the summary bullet never made it in.\n\n"
                    "- [rationale] The old cache cluster was out of support\n"
                    "- [priority] high",
                    tags="delivery,decision",
                    engram_type="decision",
                    metadata={"owner": "lena"},
                ),
                engram(
                    "Undocumented vendor swap",
                    "A decision captured without its owner and with a made-up priority.\n\n"
                    "- [summary] The payments vendor was swapped during the outage\n"
                    "- [priority] urgent",
                    tags="delivery,decision",
                    engram_type="decision",
                ),
                engram(
                    "Playbook Schema",
                    "The shape for playbook engrams in this domain, enforced strictly.\n\n"
                    "- [convention] Playbooks state an objective and numbered steps #delivery\n"
                    "- [convention] Every playbook names its owner #delivery",
                    tags="schema,delivery",
                    engram_type="schema",
                    metadata={
                        "entity": "playbook",
                        "version": 1,
                        "schema": {
                            "objective": "string, what the playbook achieves",
                            "step(array)": "string",
                        },
                        "settings": {
                            "validation": "strict",
                            "frontmatter": {"owner": "string"},
                        },
                    },
                ),
                engram(
                    "Rollback playbook",
                    "How to take a bad release back out.\n\n"
                    "- [objective] Restore the previous release within ten minutes\n"
                    "- [step] Freeze the deploy pipeline\n"
                    "- [step] Re-promote the previous color\n"
                    "- [step] Confirm error rates return to baseline",
                    tags="delivery,playbook",
                    engram_type="playbook",
                    metadata={"owner": "miguel"},
                ),
                engram(
                    "Incident comms playbook",
                    "Who says what during an incident.\n\n"
                    "- [objective] Keep stakeholders informed without distracting responders\n"
                    "- [step] Open a dedicated incident channel\n"
                    "- [step] Post a status update every thirty minutes",
                    tags="delivery,playbook",
                    engram_type="playbook",
                    metadata={"owner": "priya"},
                ),
                engram(
                    "Dependency upgrade playbook",
                    "How routine upgrades roll through the fleet.\n\n"
                    "- [objective] Upgrade shared dependencies without breaking consumers\n"
                    "- [step] Upgrade the canary service first\n"
                    "- [step] Watch its error budget for a full day\n"
                    "- [step] Roll the remaining services in dependency order",
                    tags="delivery,playbook",
                    engram_type="playbook",
                    metadata={"owner": "dana"},
                ),
                engram(
                    "Retro Schema",
                    "The shape for retro engrams, still in its adoption phase.\n\n"
                    "- [convention] Retros record what went well and what needs work #delivery\n"
                    "- [convention] Action items are optional but encouraged #delivery",
                    tags="schema,delivery",
                    engram_type="schema",
                    metadata={
                        "entity": "retro",
                        "version": 1,
                        "schema": {
                            "went_well": "string",
                            "needs_work": "string",
                            "action?(array)": "string",
                        },
                        "settings": {"validation": "warn"},
                    },
                ),
                engram(
                    "Q1 latency retro",
                    "Looking back at the latency push.\n\n"
                    "- [went_well] P99 latency halved without a rollback\n"
                    "- [needs_work] Load test coverage lagged the changes\n"
                    "- [action] Add a load test stage to the deploy pipeline",
                    tags="delivery,retro",
                    engram_type="retro",
                ),
                engram(
                    "Launch retro",
                    "Looking back at the spring launch.\n\n"
                    "- [went_well] The launch checklist caught two blockers early\n"
                    "- [needs_work] Support was looped in a week too late",
                    tags="delivery,retro",
                    engram_type="retro",
                ),
                engram(
                    "Oncall retro",
                    "Looking back at the rotation change.\n\n"
                    "- [went_well] Page volume dropped after the alert cleanup\n"
                    "- [needs_work] Handover notes were inconsistent\n"
                    "- [action] Adopt a handover template",
                    tags="delivery,retro",
                    engram_type="retro",
                ),
                engram(
                    "Incident 2026-03 payment lag",
                    "A queue depth incident.\n\n"
                    "- [fact] Payment confirmations lagged by four minutes at peak\n"
                    "- [lesson] Queue depth alerts fired too late to matter",
                    tags="delivery,incident",
                    engram_type="incident",
                    metadata={"owner": "lena"},
                ),
                engram(
                    "Incident 2026-04 cache storm",
                    "A thundering herd incident.\n\n"
                    "- [fact] A cache flush stampeded the primary database\n"
                    "- [lesson] Cache flushes need jittered expiry",
                    tags="delivery,incident",
                    engram_type="incident",
                    metadata={"owner": "dana"},
                ),
                engram(
                    "Incident 2026-05 dns flap",
                    "A resolver incident.\n\n"
                    "- [fact] Internal DNS flapped for eleven minutes during the resolver upgrade\n"
                    "- [lesson] Resolver upgrades belong in the maintenance window",
                    tags="delivery,incident",
                    engram_type="incident",
                    metadata={"owner": "miguel"},
                ),
                engram(
                    "Incident 2026-06 queue backlog",
                    "A consumer stall incident.\n\n"
                    "- [fact] A stalled consumer group backed the event queue up for an hour\n"
                    "- [lesson] Consumer lag needs its own alert, not just queue depth",
                    tags="delivery,incident",
                    engram_type="incident",
                    metadata={"owner": "priya"},
                ),
                engram(
                    "Incident 2026-06 cert expiry",
                    "An expiry incident.\n\n"
                    "- [fact] An internal certificate expired unnoticed and broke service auth\n"
                    "- [lesson] Certificate expiry belongs on the rotation calendar",
                    tags="delivery,incident",
                    engram_type="incident",
                    metadata={"owner": "lena"},
                ),
            ],
        },
    },
    # Linked product knowledge plus a near-empty distractor domain: tests
    # build_context navigation and structure questions where a MANIFEST
    # read is the right move.
    "nimbus": {
        "product-atlas": {
            "scope": [
                "The Atlas product: sync engine, conflict resolution, offline mode",
                "Sharing model and search behavior",
            ],
            "when": [
                "Questions about how Atlas syncs, shares, searches or behaves offline",
            ],
            "notes": ["Engrams here are densely linked; build_context pays off"],
            "engrams": [
                engram(
                    "Sync engine overview",
                    "How Atlas keeps replicas in agreement.\n\n"
                    "- [fact] Atlas sync is CRDT based with per-field merge #product-atlas\n"
                    "- depends_on [[Conflict Resolution]]\n"
                    '- "relates to" [[Offline Mode]]',
                    tags="product-atlas,sync",
                    engram_type="architecture",
                ),
                engram(
                    "Conflict resolution",
                    "How Atlas settles concurrent edits.\n\n"
                    "- [fact] Conflicts resolve last-writer-wins per field using vector clocks #product-atlas\n"
                    "- [fact] Deletes win over concurrent edits to the same field #product-atlas",
                    tags="product-atlas,sync",
                ),
                engram(
                    "Offline mode",
                    "How long Atlas tolerates being offline.\n\n"
                    "- [fact] Atlas runs 30 days offline before it requires re-authentication #product-atlas\n"
                    "- depends_on [[Sync Engine Overview]]",
                    tags="product-atlas,offline",
                ),
                engram(
                    "Sharing model",
                    "How Atlas documents are shared.\n\n"
                    "- [fact] Sharing is link based with three roles: viewer, editor and owner #product-atlas\n"
                    "- [fact] Only owners can revoke share links #product-atlas",
                    tags="product-atlas,sharing",
                ),
                engram(
                    "Search quirks",
                    "A known lag between sync and search.\n\n"
                    "- [gotcha] The search index lags sync by up to 60 seconds #product-atlas\n"
                    '- "relates to" [[Sync Engine Overview]]',
                    tags="product-atlas,search,gotcha",
                ),
            ],
        },
        "scratch": {
            "scope": ["Temporary experiments without a home yet"],
            "when": ["Rarely; only for scratch experiments"],
            "notes": [],
            "engrams": [
                engram(
                    "Scratch note",
                    "A placeholder entry.\n\n"
                    "- [idea] Placeholder for experiments that have not found a home #scratch\n"
                    "- [idea] Anything here may be deleted without notice #scratch",
                    tags="scratch",
                    status="idea",
                ),
            ],
        },
    },
    # Strategic-initiative knowledge base for the retrieval-depth items:
    # every load-bearing fact sits deep in an engram body, past the first
    # 200 characters and away from the vocabulary a natural query would
    # use, so no search snippet (a 70/140-char window around the first
    # term match, or a 200-char lead-in) can surface it. The initiative
    # name and the generic concept nouns live in each title and lead
    # paragraph, pinning any matched window to the shallow text. The
    # platform overview deliberately carries a stale illustrative claim
    # in its lead that only the strategy narrative corrects.
    "aurora": {
        "aurora": {
            "scope": [
                "The Aurora program: strategy, platform architecture and delivery phases",
                "Why the pivot exists, who buys it and what ships when",
            ],
            "when": [
                "Any question about Aurora: its strategy, buyers, platform, phases or competitors",
            ],
            "notes": [
                "The strategy narrative is authoritative where documents disagree",
            ],
            "engrams": [
                engram(
                    "Strategy narrative",
                    "The strategic rationale for Aurora, the company's pivot "
                    "from selling standalone dashboards to operating a "
                    "governed insight platform. This narrative records why "
                    "the program exists, who buys it, which capability ships "
                    "first, how many connectors are in scope and what would "
                    "make us stop.\n\n"
                    "Approved by the leadership team in the spring cycle, "
                    "superseding the earlier concept memo. The bullets below "
                    "are decisions, not aspirations.\n\n"
                    "- [decision] The purchase call sits with a three-seat committee: the operations chair, the finance owner and the technology validator #strategy\n"
                    "- [decision] The bet is walked back if fewer than three lighthouse customers renew by month eighteen #strategy\n"
                    "- [decision] The road not taken: bolt analytics onto our older suite and ride the installed base for another decade #strategy\n"
                    "- [decision] The first build focus is governed recovery with outcome-based planning, superseding the illustrative list in the platform overview #strategy\n"
                    "- [decision] Opening scope trims the connectors to four sources, not the dozen sketched in the platform overview #strategy",
                    tags="aurora,strategy",
                    engram_type="strategy",
                ),
                engram(
                    "Platform architecture",
                    "An early sketch of the Aurora platform. As a working "
                    "illustration this overview shows scenario budgeting as "
                    "the first capability and twelve source connectors at "
                    "launch; the strategy narrative holds the committed "
                    "scope. Four layers move signals from ingestion to "
                    "reasoning and on to reallocation decisions.\n\n"
                    "The layers below are stable even where the capability "
                    "list above is not.\n\n"
                    "- [fact] The ingestion layer normalizes execution signals from work trackers into the outcome ledger #architecture\n"
                    "- [fact] The outcome ledger is the platform's core asset: an append-only record linking each decision to what actually happened #architecture\n"
                    "- [fact] The reasoning layer replays past decisions against the ledger to price each reallocation move #architecture\n"
                    "- refines [[Strategy narrative]]",
                    tags="aurora,architecture",
                    engram_type="architecture",
                    status="draft",
                ),
                engram(
                    "Phase plan",
                    "The phased delivery plan for Aurora, from first "
                    "shipment through platform buildout. Three phases pace "
                    "the program: what ships first, who receives it and the "
                    "gate each phase must clear before the next begins.\n\n"
                    "Dates move with capacity; the gates and the order do "
                    "not. Each phase closes with a written review before the "
                    "next opens, re-baselined quarterly with the program "
                    "board.\n\n"
                    "- [decision] The opening phase ships governed recovery to the five design partners before any platform work #plan\n"
                    "- [fact] The second phase begins only after the outcome ledger holds two full quarters of signals #plan\n"
                    "- [fact] The third phase lets agents propose reallocation moves under human approval #plan",
                    tags="aurora,plan",
                    engram_type="plan",
                ),
                engram(
                    "Competitive watch",
                    "Tracking rivals and adjacent platforms around Aurora. "
                    "Standing watch notes on who moves toward our buyers, "
                    "our category and our positioning; refreshed as material "
                    "events land rather than on a schedule.\n\n"
                    "Raw press coverage stays out; only moves that change "
                    "our posture get recorded here, with the source noted "
                    "for each entry.\n\n"
                    "- [fact] The nearest rival shipped a positioning paper aimed straight at our buying committee in April #strategy\n"
                    "- [fact] Two adjacent platform vendors are courting the same operations chairs we sell to #strategy",
                    tags="aurora,strategy",
                ),
            ],
        },
        "workbench": {
            "scope": ["Personal working notes, logistics and scratch material"],
            "when": ["Offsite logistics, meeting notes and other scratch lookups"],
            "notes": ["Nothing here is Aurora program knowledge"],
            "engrams": [
                engram(
                    "Offsite logistics",
                    "Notes for the spring offsite.\n\n"
                    "- [fact] The offsite venue holds forty people #workbench\n"
                    "- [fact] Catering needs final numbers a week ahead #workbench",
                    tags="workbench",
                ),
            ],
        },
    },
}


def run(cmd: list[str], env: dict) -> None:
    proc = subprocess.run(
        cmd, capture_output=True, text=True, encoding="utf-8", env=env
    )
    if proc.returncode != 0:
        raise SystemExit(
            f"command failed ({' '.join(cmd[:6])} ...):\n{proc.stderr.strip()}"
        )


def patch_manifest(path: Path, name: str, spec: dict) -> None:
    text = path.read_text(encoding="utf-8")
    front, _, _ = text.partition(f"# {name}")

    def bullets(lines: list[str]) -> str:
        return "\n".join(f"- {line}" for line in lines) if lines else "- (none)"

    body = (
        f"# {name}\n\n"
        f"## Scope\n\n{bullets(spec['scope'])}\n\n"
        f"## When to Use\n\n{bullets(spec['when'])}\n\n"
        f"## Notes for Agents\n\n{bullets(spec['notes'])}\n"
    )
    path.write_text(front + body, encoding="utf-8")


def build_workspace(name: str, domains: dict, state_root: Path) -> None:
    ws_dir = WORKSPACES_ROOT / name
    build = state_root / name
    build.mkdir(parents=True)
    env = dict(
        os.environ,
        XDG_STATE_HOME=str(build / "state"),
        XDG_CONFIG_HOME=str(build / "xdg-config"),
    )
    config = build / "config.yaml"
    db = build / "index.db"

    for domain_name, spec in domains.items():
        domain_dir = ws_dir / "domains" / domain_name
        domain_dir.mkdir(parents=True)
        run(
            [CRYSTALLINE_BIN, "domain", "init", str(domain_dir), "--name", domain_name],
            env,
        )
        patch_manifest(domain_dir / "MANIFEST.md", domain_name, spec)
        run(
            [
                CRYSTALLINE_BIN, "--db", str(db), "domain", "add",
                domain_name, str(domain_dir), "--config", str(config),
            ],
            env,
        )
        for item in spec["engrams"]:
            cmd = [
                CRYSTALLINE_BIN, "--db", str(db), "write",
                domain_name, item["title"],
                "--content", item["content"],
                "--config", str(config),
            ]
            if item["tags"]:
                cmd.extend(["--tags", item["tags"]])
            if item["status"]:
                cmd.extend(["--status", item["status"]])
            if item["type"]:
                cmd.extend(["--type", item["type"]])
            if item["metadata"]:
                cmd.extend(["--metadata", json.dumps(item["metadata"])])
            run(cmd, env)

        verify = subprocess.run(
            [CRYSTALLINE_BIN, "verify", str(domain_dir)],
            capture_output=True, text=True, encoding="utf-8", env=env,
        )
        if verify.returncode != 0:
            raise SystemExit(
                f"crystalline verify failed for {name}/{domain_name}:\n"
                f"{verify.stdout}\n{verify.stderr}"
            )
        count = len(spec["engrams"])
        print(f"  {name}/{domain_name}: {count} engrams, verify clean")


def main() -> None:
    if not Path(CRYSTALLINE_BIN).exists():
        raise SystemExit(
            f"crystalline binary not found at {CRYSTALLINE_BIN}; "
            "run `cargo build --release` first or set CRYSTALLINE_BIN"
        )
    names = sys.argv[1:]
    unknown = sorted(set(names) - set(WORKSPACES))
    if unknown:
        raise SystemExit(f"unknown workspace(s): {', '.join(unknown)}")
    if names:
        targets = {name: WORKSPACES[name] for name in names}
        for name in targets:
            shutil.rmtree(WORKSPACES_ROOT / name, ignore_errors=True)
    else:
        targets = WORKSPACES
        if WORKSPACES_ROOT.exists():
            shutil.rmtree(WORKSPACES_ROOT)
    with tempfile.TemporaryDirectory(prefix="cst-fixture-build-") as state_root:
        for name, domains in targets.items():
            build_workspace(name, domains, Path(state_root))
    print(f"fixtures written to {WORKSPACES_ROOT}")


if __name__ == "__main__":
    sys.exit(main())
