#!/usr/bin/env python3
"""Generate a ten-thousand-engram corpus for the scalability test.

Five domains of two thousand engrams each, written as markdown on disk in
twenty folders per domain, deterministic from a seed. The conventions follow
`evals/skill-training/fixtures/generate.py`: a MANIFEST.md with Scope and
When to Use at every domain root, engram files with the four required
frontmatter fields plus tags and a pinned `recorded_at`, observation bullets
`- [category] text #tag` and relation bullets `- relation [[Title]]`.

Unlike that generator this one writes the files itself rather than driving the
binary: ten thousand `crystalline write` calls would take longer than the test
they feed. The shape is checked afterwards with `crystalline verify`.

Body length follows the distribution the spec asks for, measured the way
verify's Q002 measures it (`body.chars() / 4`):

    70 percent   200 to 800 tokens
    25 percent   800 to 2,000 tokens
    4 percent    2,000 to 2,500 tokens
    1 percent    over 2,500 tokens, so the oversized rule fires a little

Usage:

    python3 evals/scale/generate.py --out evals/scale/corpus
    python3 evals/scale/generate.py --seed 7 --domains 2 --engrams-per-domain 50
"""

from __future__ import annotations

import argparse
import json
import random
import re
import shutil
import sys
from datetime import date, timedelta
from pathlib import Path

# --- the corpus shape --------------------------------------------------------

# Five domains, each with its own MANIFEST voice and its own folder layout.
# The names are ordinary operations and research topics: the corpus models the
# work corpus that prompted this test, not any particular product.
DOMAINS: list[dict] = [
    {
        "name": "platform",
        "scope": [
            "Service platform: ingestion, queues, storage and the request path",
            "How the platform is deployed, observed and rolled back",
        ],
        "when": [
            "Questions about the request path, queues, storage engines or deploys",
            "Anything about platform capacity, latency budgets or incident handling",
        ],
        "notes": [
            "Team rosters and on-call compensation live with people operations",
        ],
        "folders": [
            "ingestion",
            "queues",
            "storage",
            "request-path",
            "caching",
            "deploys",
            "observability",
            "incidents",
            "capacity",
            "networking",
            "security",
            "migrations",
            "runbooks",
            "decisions",
            "postmortems",
            "interfaces",
            "scheduling",
            "configuration",
            "testing",
            "sources",
        ],
    },
    {
        "name": "observatory",
        "scope": [
            "Telescope operations: optics, dome, calibration and the nightly routine",
            "Instrument characterisation and the data reduction pipeline",
        ],
        "when": [
            "Questions about observing runs, calibration, seeing or instrument health",
            "Anything about the reduction pipeline and its data products",
        ],
        "notes": [
            "Proposal deadlines and time allocation are tracked elsewhere",
        ],
        "folders": [
            "optics",
            "dome",
            "calibration",
            "detectors",
            "spectroscopy",
            "photometry",
            "guiding",
            "weather",
            "scheduling",
            "pipeline",
            "archive",
            "maintenance",
            "safety",
            "runbooks",
            "decisions",
            "lessons",
            "instruments",
            "site",
            "software",
            "sources",
        ],
    },
    {
        "name": "harbor",
        "scope": [
            "Port operations: berths, cranes, yard planning and customs",
            "Vessel scheduling, pilotage and the tidal window",
        ],
        "when": [
            "Questions about berth allocation, crane productivity or yard moves",
            "Anything about customs clearance, manifests or hazardous cargo",
        ],
        "notes": [
            "Crew welfare and shore leave are handled by the seafarers office",
        ],
        "folders": [
            "berths",
            "cranes",
            "yard",
            "customs",
            "pilotage",
            "tides",
            "manifests",
            "hazardous",
            "rail",
            "trucking",
            "maintenance",
            "safety",
            "weather",
            "runbooks",
            "decisions",
            "lessons",
            "contracts",
            "billing",
            "reporting",
            "sources",
        ],
    },
    {
        "name": "meridian",
        "scope": [
            "Clinical study operations: protocols, sites, consent and data capture",
            "Sample handling, laboratory workflow and quality review",
        ],
        "when": [
            "Questions about protocol amendments, site readiness or consent flow",
            "Anything about sample chain of custody, assays or query resolution",
        ],
        "notes": [
            "Regulatory submissions themselves live with the regulatory group",
        ],
        "folders": [
            "protocols",
            "sites",
            "consent",
            "capture",
            "samples",
            "laboratory",
            "assays",
            "monitoring",
            "queries",
            "quality",
            "training",
            "supplies",
            "statistics",
            "runbooks",
            "decisions",
            "lessons",
            "safety",
            "reporting",
            "vendors",
            "sources",
        ],
    },
    {
        "name": "atelier",
        "scope": [
            "Production workshop: materials, tooling, finishing and the shop floor",
            "Supplier relationships, costing and the repair practice",
        ],
        "when": [
            "Questions about materials, jigs, finishing schedules or shop capacity",
            "Anything about suppliers, lead times, costing or repairs",
        ],
        "notes": [
            "Retail pricing and the shop window are the storefront team's",
        ],
        "folders": [
            "materials",
            "tooling",
            "jigs",
            "finishing",
            "assembly",
            "quality",
            "suppliers",
            "costing",
            "repairs",
            "shipping",
            "storage",
            "safety",
            "training",
            "runbooks",
            "decisions",
            "lessons",
            "machines",
            "patterns",
            "waste",
            "sources",
        ],
    },
]

ENGRAM_TYPES = [
    ("engram", 60),
    ("guide", 10),
    ("decision", 10),
    ("architecture", 8),
    ("runbook", 7),
    ("reference", 5),
]

STATUSES = [
    ("stable", 80),
    ("implemented", 6),
    ("draft", 5),
    ("proposed", 3),
    ("deprecated", 3),
    ("superseded", 2),
    ("archived", 1),
]

CATEGORIES = [
    "fact",
    "decision",
    "pattern",
    "gotcha",
    "convention",
    "lesson",
    "risk",
    "insight",
    "idea",
    "proposal",
]

RELATIONS = [
    "depends_on",
    "refines",
    "part_of",
    "supersedes",
    "summarizes",
    '"relates to"',
    '"follows from"',
    "implements",
    "contrasts_with",
    "extends",
]

# Two hundred tags, the fixed vocabulary every engram draws three to six from.
TAGS = [
    "ingestion", "throughput", "latency", "backpressure", "retry", "queue",
    "storage", "indexing", "cache", "eviction", "sharding", "replication",
    "failover", "deploy", "rollback", "canary", "observability", "tracing",
    "metrics", "logging", "alerting", "incident", "postmortem", "capacity",
    "scaling", "networking", "routing", "firewall", "encryption", "secrets",
    "authentication", "authorisation", "audit", "migration", "schema",
    "backfill", "runbook", "checklist", "convention", "decision", "lesson",
    "risk", "gotcha", "pattern", "insight", "interface", "contract",
    "versioning", "compatibility", "deprecation", "scheduling", "batching",
    "streaming", "idempotency", "consistency", "durability", "recovery",
    "snapshot", "restore", "testing", "fixtures", "benchmark", "profiling",
    "memory", "cpu", "disk", "bandwidth", "cost", "budget", "vendor",
    "procurement", "optics", "mirror", "coating", "collimation", "focus",
    "dome", "shutter", "seeing", "extinction", "photometry", "spectroscopy",
    "detector", "readout", "darks", "flats", "bias", "calibration",
    "guiding", "tracking", "pointing", "ephemeris", "weather", "humidity",
    "wind", "pipeline", "reduction", "astrometry", "archive", "catalogue",
    "berth", "crane", "yard", "container", "manifest", "customs", "clearance",
    "pilotage", "tug", "tide", "draft-limit", "hazardous", "reefer", "rail",
    "trucking", "gate", "turnaround", "demurrage", "stowage", "lashing",
    "bunkering", "dredging", "mooring", "protocol", "amendment", "site",
    "consent", "enrolment", "randomisation", "blinding", "dosing",
    "adverse-event", "sample", "aliquot", "centrifuge", "freezer",
    "chain-of-custody", "assay", "plate", "reagent", "control-sample",
    "monitoring", "query", "source-data", "deviation", "quality-review",
    "training", "supplies", "shipment", "statistics", "endpoint", "materials",
    "timber", "steel", "resin", "adhesive", "abrasive", "tooling", "jig",
    "fixture", "lathe", "mill", "press", "finishing", "lacquer", "polish",
    "assembly", "tolerance", "inspection", "rework", "supplier", "lead-time",
    "costing", "margin", "repair", "warranty", "packaging", "shipping",
    "storage-shelf", "humidity-control", "safety", "ppe", "lockout",
    "apprentice", "waste", "recycling", "energy", "maintenance", "wear",
    "lubrication", "downtime", "spare-part", "documentation", "handover",
    "onboarding", "glossary", "reference", "source", "history",
]

# Title parts. The product is Subject + Facet + Kind, sampled without
# replacement per domain so two thousand titles never collide, and screened so
# no title ever carries a colon or a slash (both break link resolution).
SUBJECTS = [
    "Retry queue", "Ingest worker", "Write path", "Read replica", "Object store",
    "Chunk index", "Session cache", "Edge router", "Batch scheduler",
    "Migration runner", "Dome shutter", "Filter wheel", "Guide camera",
    "Primary mirror", "Spectrograph slit", "Flat field", "Weather mast",
    "Reduction pipeline", "Archive loader", "Pointing model", "Berth window",
    "Quay crane", "Yard stack", "Customs gate", "Pilot boarding",
    "Tidal window", "Reefer plug", "Rail siding", "Truck gate", "Lashing bridge",
    "Consent form", "Site file", "Sample courier", "Freezer rack",
    "Assay plate", "Monitoring visit", "Query backlog", "Supply shipment",
    "Randomisation list", "Deviation log", "Timber stock", "Finishing booth",
    "Assembly jig", "Grinding wheel", "Lacquer batch", "Supplier contract",
    "Repair bench", "Packing line", "Dust extraction", "Apprentice rota",
]

FACETS = [
    "backpressure", "throughput", "failure mode", "capacity plan",
    "cutover", "handover", "calibration", "drift", "tolerance", "budget",
    "latency profile", "retention", "escalation path", "ownership",
    "validation", "recovery", "warm-up", "shutdown", "inspection",
    "sequencing", "staffing", "audit trail", "cost model", "spare holding",
    "alert threshold", "dependency map", "change window", "rollback plan",
    "acceptance test", "pilot run", "seasonal variation", "night shift",
    "wet weather", "peak week", "cold start", "quiet period",
    "first attempt", "second attempt", "long tail", "edge case",
]

KINDS = [
    "notes", "runbook", "decision", "review", "reference", "summary",
    "conventions", "gotchas", "checklist", "history", "measurements",
    "walkthrough", "postmortem", "proposal", "standard", "briefing",
    "inventory", "schedule", "comparison", "digest",
]

# Sentence stock. Each template is filled from the term banks below, so the
# prose is varied enough for a search battery to discriminate between engrams
# while staying entirely reproducible from the seed.
TEMPLATES = [
    "The {thing} is sized for {measure} and anything past that is shed rather than queued.",
    "Every {thing} is checked against {measure} before the {actor} signs the shift off.",
    "{Actor} owns the {thing}; changes to it go through the {process} without exception.",
    "The {process} takes about {duration} end to end, most of it waiting on the {thing}.",
    "A {thing} that misses {measure} is pulled out of service and handed to {actor}.",
    "The {thing} was rebuilt after the {event}, which is why the {process} looks the way it does.",
    "Nothing about the {thing} is automatic: the {actor} still walks the {process} by hand.",
    "Two {thing} runs in a row over {measure} trigger the {process}, and that has happened twice.",
    "The cheapest fix for a slow {thing} is to raise {measure} first and only then touch the {process}.",
    "{Actor} keeps a written record of every {thing} exception, filed under the {process}.",
    "The {event} taught us that the {thing} degrades quietly rather than failing outright.",
    "Under {condition} the {thing} holds {measure}; outside it the numbers stop meaning anything.",
    "The {process} exists because the {thing} used to be changed without anybody noticing.",
    "Budget about {duration} for the {process} whenever the {thing} has been idle.",
    "A {thing} reading outside {measure} is a symptom, and the cause is almost always upstream.",
    "The {actor} and the {actor2} disagree about the {thing}; the {process} settles it.",
    "Do not trust the {thing} during {condition}: the reading lags the reality by {duration}.",
    "After the {event} the {process} gained a second pair of eyes on every {thing} change.",
    "The {thing} is the one place where {measure} is enforced rather than merely recorded.",
    "Replacing a {thing} is a {duration} job under {condition} and twice that in the dark.",
    "Keep the {thing} within {measure}; the {process} assumes it and nothing else checks.",
    "The {process} is documented here because the {actor} is the only one who has run it.",
]

THINGS = [
    "retry batch", "ingest lane", "write buffer", "replica lag", "chunk pass",
    "cache tier", "routing table", "schedule slot", "migration step",
    "shutter cycle", "filter change", "guide loop", "mirror coat",
    "slit alignment", "flat sequence", "wind reading", "reduction stage",
    "archive load", "pointing check", "berth turn", "crane move",
    "yard shuffle", "customs hold", "pilot transfer", "tide slot",
    "reefer check", "rail cut", "gate pass", "lashing round",
    "consent packet", "site binder", "courier run", "freezer sweep",
    "assay plate", "monitor visit", "query batch", "supply drop",
    "timber lot", "finish coat", "assembly jig", "wheel dressing",
    "lacquer mix", "supplier call", "repair ticket", "packing run",
]

MEASURES = [
    "a two minute ceiling", "ninety five percent coverage", "four gigabytes",
    "a ten second budget", "thirty degrees of swing", "one arcsecond",
    "twelve hours of headroom", "a fifteen percent margin",
    "a hundred units an hour", "half a millimetre", "two degrees of tilt",
    "an eight hour window", "forty decibels", "a five percent reject rate",
    "three sigma", "sixty items per crate", "a one day lead time",
    "a hundred and twenty seconds", "eleven percent waste",
]

ACTORS = [
    "duty engineer", "night operator", "shift lead", "quality reviewer",
    "yard planner", "study monitor", "bench technician", "line supervisor",
    "data steward", "maintenance fitter", "platform owner", "site coordinator",
]

PROCESSES = [
    "change review", "handover routine", "weekly audit", "escalation ladder",
    "acceptance run", "morning walk", "sign-off checklist", "standing order",
    "calibration cycle", "inspection pass", "reconciliation", "dry run",
]

DURATIONS = [
    "twenty minutes", "an hour", "half a shift", "two days", "a week",
    "ninety seconds", "three hours", "a single night", "four working days",
]

EVENTS = [
    "winter outage", "spring backlog", "audit of 2025", "vendor change",
    "third attempt", "relocation", "capacity crunch",
    "quiet fortnight", "failed cutover", "storm week",
]

CONDITIONS = [
    "high humidity", "peak load", "a cold start", "reduced staffing",
    "a partial outage", "the night shift", "heavy rain", "month end",
]

# --- planted contradictions (--contradictions) --------------------------------
#
# Each planted item is two engrams in a `contradictions/` folder of one domain
# that share a title stem and an opening paragraph (so their lead embeddings
# are related) and differ in one observation line. A flip must be read as a
# contradiction; a hard negative must stay quiet. The sidecar records every
# item with its line numbers, so the evaluator can find the planted line pair.

# (English subject, German subject with its article)
PLANT_SUBJECTS = [
    ("nightly backup", "das nächtliche Backup"),
    ("build server", "der Build-Server"),
    ("warehouse system", "das Lagerverwaltungssystem"),
    ("cold chain monitor", "die Kühlkettenüberwachung"),
    ("night run", "der Nachtlauf"),
    ("test bench", "der Prüfstand"),
    ("ingest service", "der Ingest-Dienst"),
    ("release gate", "das Freigabetor"),
]

# (type, lang, line a, line b); {s} is the English subject, {D} the German one
# capitalized, {d} the German one as written.
PLANT_FLIPS = [
    ("negation", "en", "The {s} runs on a fixed schedule", "The {s} does not run on a fixed schedule"),
    ("negation", "de", "{D} läuft nach festem Zeitplan", "{D} läuft nicht nach festem Zeitplan"),
    ("negation", "de", "{D} braucht keinen Neustart", "{D} braucht einen Neustart"),
    ("negation", "de", "{D} nutzt nicht mehr das alte Skript", "{D} nutzt das alte Skript"),
    ("number", "en", "The {s} keeps 1,000 entries", "The {s} keeps 1,500 entries"),
    ("number", "de", "{D} fasst 1.000 Einträge", "{D} fasst 1.500 Einträge"),
    ("number", "de", "Die Toleranz für {d} liegt bei 1,5 mm", "Die Toleranz für {d} liegt bei 2,5 mm"),
    ("version", "en", "The {s} uses Node 18", "The {s} uses Node 20"),
    ("version", "en", "The {s} uses Node 18.19.0", "The {s} uses Node 20.1.0"),
    ("date", "en", "The {s} was introduced in 2023", "The {s} was introduced in 2021"),
    ("date", "de", "Die Abnahme für {d} war am 27.09.2026", "Die Abnahme für {d} war am 14.03.2025"),
    ("antonym", "en", "The {s} is enabled by default", "The {s} is disabled by default"),
    ("antonym", "de", "{D} ist standardmäßig aktiviert", "{D} ist standardmäßig deaktiviert"),
    ("entity", "en", "The {s} is owned by the platform team", "The {s} is owned by the data team"),
    ("entity", "de", "{D} gehört dem Plattformteam", "{D} gehört dem Datenteam"),
    ("quantity", "en", "The {s} always needs a manual approval", "The {s} never needs a manual approval"),
    ("quantity", "de", "{D} braucht immer eine manuelle Freigabe", "{D} braucht nie eine manuelle Freigabe"),
    ("number", "mixed", "The {s} uses version 18", "{D} nutzt Version 20"),
    ("antonym", "mixed", "The {s} is enabled by default", "{D} ist standardmäßig deaktiviert"),
    ("negation", "mixed", "{D} braucht keinen Neustart", "The {s} needs a restart"),
    ("quantity", "mixed", "The {s} always needs a manual approval", "{D} braucht nie eine manuelle Freigabe"),
]

PLANT_NEGATIVES = [
    ("compatible", "en", "The {s} runs at night", "The {s} writes a log file"),
    ("compatible", "de", "{D} läuft nachts", "{D} schreibt eine Logdatei"),
    ("scope", "en", "On the work mac the {s} uses version 18", "On the home mac the {s} uses version 20"),
    ("scope", "de", "Auf dem Arbeits-Mac nutzt {d} Version 18", "Auf dem Heim-Mac nutzt {d} Version 20"),
    ("period-undated", "en", "The {s} used version 18", "The {s} uses version 20"),
    ("period-undated", "de", "{D} nutzte Version 18", "{D} nutzt Version 20"),
    ("period-dated", "en", "Until 2024 the {s} used version 18", "Since 2025 the {s} uses version 20"),
    ("period-dated", "de", "Bis 2024 nutzte {d} Version 18", "Seit 2025 nutzt {d} Version 20"),
    ("paraphrase", "en", "The {s} runs every night", "Every night the {s} runs"),
    ("paraphrase", "de", "{D} läuft jede Nacht", "Jede Nacht läuft {d}"),
    ("entailment", "en", "The {s} runs every night at two", "The {s} runs every night"),
    ("entailment", "de", "{D} läuft jede Nacht um zwei", "{D} läuft jede Nacht"),
    ("compatible", "mixed", "The {s} runs at night", "{D} schreibt eine Logdatei"),
]


def plant_lang(index: int) -> str:
    """Half English, half German, one in ten mixed."""
    if index % 10 == 9:
        return "mixed"
    return "en" if index % 2 == 0 else "de"


CATEGORY_TEMPLATES = {
    "fact": "The {thing} holds {measure} under {condition}",
    "decision": "We settled on the {process} for every {thing} change",
    "pattern": "A {thing} that drifts is re-seated before it is replaced",
    "gotcha": "The {thing} reports success while still {duration} behind",
    "convention": "Every {thing} change is announced in the {process} first",
    "lesson": "The {event} showed that the {thing} needs {measure} of headroom",
    "risk": "A {thing} left unchecked through {condition} can cost {duration}",
    "insight": "The {thing} is the constraint, not the {process} around it",
    "idea": "The {process} could take the {thing} readings automatically",
    "proposal": "Give the {actor} a second {thing} so the {process} never waits",
}

TOKEN_BUDGET = 2500
CHARS_PER_TOKEN = 4

# Write provenance, pinned like every other date here. A file written by hand
# rather than through a write tool carries none, and verify's T006 says so on
# every one of ten thousand files, which would bury the findings this corpus
# exists to produce.
GENERATED = "{ by: process:crystalline-scale-generator, at: 2026-09-01T09:00:00+00:00 }"

# Pinned so an age-based rule measures a constant instead of the days since
# this script last ran. Dates are spread backwards from here.
RECORDED_LATEST = date(2026, 6, 30)
RECORDED_SPAN_DAYS = 900

# --- helpers -----------------------------------------------------------------


def weighted(rng: random.Random, pairs: list[tuple[str, int]]) -> str:
    total = sum(w for _, w in pairs)
    roll = rng.randrange(total)
    for value, weight in pairs:
        if roll < weight:
            return value
        roll -= weight
    return pairs[-1][0]


def slugify(text: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    return slug


def titles_for(rng: random.Random, count: int) -> list[str]:
    """`count` distinct titles, none carrying a colon or a slash."""
    seen: set[str] = set()
    out: list[str] = []
    while len(out) < count:
        title = f"{rng.choice(SUBJECTS)} {rng.choice(FACETS)} {rng.choice(KINDS)}"
        if ":" in title or "/" in title:
            continue
        if title in seen:
            continue
        seen.add(title)
        out.append(title)
    return out


def sentence(rng: random.Random) -> str:
    template = rng.choice(TEMPLATES)
    actor = rng.choice(ACTORS)
    actor2 = rng.choice([a for a in ACTORS if a != actor])
    return template.format(
        thing=rng.choice(THINGS),
        measure=rng.choice(MEASURES),
        actor=actor,
        actor2=actor2,
        Actor=actor[0].upper() + actor[1:],
        process=rng.choice(PROCESSES),
        duration=rng.choice(DURATIONS),
        event=rng.choice(EVENTS),
        condition=rng.choice(CONDITIONS),
    )


def observation(rng: random.Random, tags: list[str]) -> str:
    category = rng.choice(CATEGORIES)
    template = CATEGORY_TEMPLATES.get(category, "The {thing} holds {measure}")
    text = template.format(
        thing=rng.choice(THINGS),
        measure=rng.choice(MEASURES),
        actor=rng.choice(ACTORS),
        process=rng.choice(PROCESSES),
        duration=rng.choice(DURATIONS),
        event=rng.choice(EVENTS),
        condition=rng.choice(CONDITIONS),
    )
    return f"- [{category}] {text} #{rng.choice(tags)}"


def target_tokens(rng: random.Random) -> int:
    roll = rng.random()
    if roll < 0.70:
        return rng.randint(200, 800)
    if roll < 0.95:
        return rng.randint(800, 2000)
    if roll < 0.99:
        return rng.randint(2000, 2500)
    return rng.randint(2560, 3200)


def manifest_text(domain: dict) -> str:
    lines = [
        "---",
        "type: manifest",
        f"title: {domain['name']}",
        "permalink: manifest",
        "tags:",
        "  - manifest",
        "  - scale-corpus",
        "status: stable",
        "recorded_at: 2026-01-05",
        f"generated: {GENERATED}",
        "---",
        "",
        f"# {domain['name']}",
        "",
        "## Scope",
        "",
    ]
    lines += [f"- {s}" for s in domain["scope"]]
    lines += ["", "## When to Use", ""]
    lines += [f"- {w}" for w in domain["when"]]
    lines += ["", "## Notes for Agents", ""]
    lines += [f"- {n}" for n in domain["notes"]]
    lines.append("")
    return "\n".join(lines)


def engram_text(
    rng: random.Random,
    title: str,
    folder: str,
    permalink: str,
    local_titles: list[str],
    foreign: tuple[str, str] | None,
) -> str:
    """One engram file: frontmatter, prose carrying its links, then bullets."""
    tags = rng.sample(TAGS, rng.randint(3, 6))
    engram_type = weighted(rng, ENGRAM_TYPES)
    status = weighted(rng, STATUSES)
    recorded = RECORDED_LATEST - timedelta(days=rng.randrange(RECORDED_SPAN_DAYS))

    # Three links on average inside the domain, one of them the relation
    # bullet, and one engram in twenty also reaches into another domain.
    link_count = rng.choice([2, 3, 3, 4])
    picks = rng.sample(local_titles, min(link_count, len(local_titles)))
    relation_target = picks[0]
    prose_links = picks[1:]

    bullets = [observation(rng, tags) for _ in range(2)]
    relation = "superseded_by" if status == "superseded" else rng.choice(RELATIONS)
    bullets.append(f"- {relation} [[{relation_target}]]")
    if foreign is not None:
        bullets.append(f'- "relates to" [[{foreign[0]}:{foreign[1]}]]')

    head = [
        "---",
        f"type: {engram_type}",
        f"title: {title}",
        f"permalink: {permalink}",
        "tags:",
    ]
    head += [f"  - {t}" for t in tags]
    head += [f"status: {status}", f"recorded_at: {recorded.isoformat()}"]

    # A small slice carries real temporal debt so the temporal sweep has
    # something to find: an elapsed validity window, or a review date past due.
    temporal_roll = rng.random()
    if temporal_roll < 0.03:
        start = recorded - timedelta(days=rng.randrange(200, 600))
        end = recorded + timedelta(days=rng.randrange(10, 120))
        head.append(f"valid_from: {start.isoformat()}")
        head.append(f"valid_to: {end.isoformat()}")
    elif temporal_roll < 0.06:
        head.append(
            f"stale_after: {(recorded + timedelta(days=rng.randrange(30, 200))).isoformat()}"
        )
    head.append(f"generated: {GENERATED}")
    head.append("---")

    tail = "\n".join(bullets)
    budget = target_tokens(rng) * CHARS_PER_TOKEN
    prose_budget = budget - len(tail) - 2

    # Filled a sentence at a time, so a body lands within a sentence of its
    # target rather than a paragraph short of it: the distribution the spec
    # asks for is only honoured if the achieved length tracks the target.
    paragraphs: list[list[str]] = [[]]
    used = 0
    link_queue = list(prose_links)
    want = rng.randint(3, 6)
    while True:
        if link_queue and len(paragraphs[-1]) == 1:
            nxt = f"The background for that sits in [[{link_queue[-1]}]]."
        else:
            nxt = sentence(rng)
        if used + len(nxt) + 1 > prose_budget:
            break
        if nxt.startswith("The background for that sits in"):
            link_queue.pop()
        paragraphs[-1].append(nxt)
        used += len(nxt) + 1
        if len(paragraphs[-1]) >= want:
            paragraphs.append([])
            want = rng.randint(3, 6)
            used += 1
    for target in link_queue:
        paragraphs.append([f"See also [[{target}]]."])

    blocks = [" ".join(p) for p in paragraphs if p]
    body = "\n\n".join(blocks) + "\n\n" + tail + "\n"
    return "\n".join(head) + "\n\n" + body


# --- generation --------------------------------------------------------------


def generate(out: Path, seed: int, domain_count: int, per_domain: int) -> dict:
    domains = DOMAINS[:domain_count]
    if len(domains) < domain_count:
        raise SystemExit(
            f"only {len(DOMAINS)} domains are defined, {domain_count} requested"
        )

    # Pass one: every title, so links can point at engrams not yet written.
    plan: dict[str, list[tuple[str, str, str]]] = {}
    for domain in domains:
        rng = random.Random(f"{seed}:titles:{domain['name']}")
        titles = titles_for(rng, per_domain)
        folders = domain["folders"]
        entries = []
        for i, title in enumerate(titles):
            folder = folders[i % len(folders)]
            entries.append((title, folder, f"{folder}/{slugify(title)}"))
        plan[domain["name"]] = entries

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    stats = {
        "domains": 0,
        "engrams": 0,
        "files": 0,
        "bytes": 0,
        "body_tokens": 0,
        "over_budget": 0,
        "buckets": [0, 0, 0, 0],
        "links": 0,
        "cross_domain_links": 0,
    }

    for domain in domains:
        name = domain["name"]
        root = out / name
        root.mkdir()
        manifest = manifest_text(domain)
        (root / "MANIFEST.md").write_text(manifest, encoding="utf-8")
        stats["files"] += 1
        stats["bytes"] += len(manifest.encode("utf-8"))
        stats["domains"] += 1

        entries = plan[name]
        others = [d["name"] for d in domains if d["name"] != name]
        for index, (title, folder, permalink) in enumerate(entries):
            rng = random.Random(f"{seed}:{name}:{index}")
            pool = [t for t, _, _ in entries if t != title]
            foreign = None
            if others and rng.random() < 0.05:
                other = rng.choice(others)
                foreign = (other, rng.choice([t for t, _, _ in plan[other]]))
            text = engram_text(rng, title, folder, permalink, pool, foreign)
            path = root / folder / f"{slugify(title)}.md"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")

            body = text.split("\n---\n", 1)[1].lstrip("\n")
            tokens = len(body) // CHARS_PER_TOKEN
            stats["engrams"] += 1
            stats["files"] += 1
            stats["bytes"] += len(text.encode("utf-8"))
            stats["body_tokens"] += tokens
            stats["links"] += text.count("[[")
            if foreign is not None:
                stats["cross_domain_links"] += 1
            if tokens > TOKEN_BUDGET:
                stats["over_budget"] += 1
            if tokens < 800:
                stats["buckets"][0] += 1
            elif tokens < 2000:
                stats["buckets"][1] += 1
            elif tokens <= TOKEN_BUDGET:
                stats["buckets"][2] += 1
            else:
                stats["buckets"][3] += 1

    return stats


PROBES = {
    "name": "probes",
    "scope": ["Planted line pairs for the contradiction measurement"],
    "when": ["Never for work: this domain exists for evals/nli/evaluate.py"],
    "notes": ["Each probe is two engrams that share a paragraph and differ in one observation line"],
}


def plant_contradictions(out: Path, seed: int, count: int) -> list:
    """Write `count` flipped pairs and `count` hard negatives into a `probes`
    domain of their own, and the sidecar. A domain of their own so the planted
    pairs never compete with the natural ones for the per-domain pair cap, and
    a seeded paragraph per probe so probes do not all read as related to each
    other. The second bullet is unique per probe (never shared between two
    probes) so it never becomes an unplanted line pair of its own."""
    rng = random.Random(f"{seed}:contradictions")
    root = out / "probes"
    root.mkdir(parents=True, exist_ok=True)
    (root / "MANIFEST.md").write_text(manifest_text(PROBES), encoding="utf-8")
    items = []
    for kind, table, stem in (("flip", PLANT_FLIPS, "Contradiction probe"), ("negative", PLANT_NEGATIVES, "Consistency probe")):
        for i in range(count):
            lang = plant_lang(i)
            ptype, _, line_a, line_b = rng.choice([t for t in table if t[1] == lang])
            en, de = PLANT_SUBJECTS[i % len(PLANT_SUBJECTS)]
            fill = {"s": en, "D": de[:1].upper() + de[1:], "d": de}
            texts = (line_a.format(**fill), line_b.format(**fill))
            domain = "probes"
            title = f"{stem} {i:04d}"
            paragraph = f"{title} records how the {en} is run. " + " ".join(
                sentence(rng) for _ in range(2)
            )
            shared = f"This is probe {i:04d} of the {stem.lower()} set, written for the contradiction measurement"
            ends = []
            for side, text in zip("ab", texts):
                side_title = f"{title} {side}"
                permalink = f"pairs/{slugify(side_title)}"
                body_lines = [
                    "---",
                    "type: engram",
                    f"title: {side_title}",
                    f"permalink: {permalink}",
                    "tags:",
                    "  - measurement",
                    "status: stable",
                    f"recorded_at: {RECORDED_LATEST.isoformat()}",
                    f"generated: {GENERATED}",
                    "---",
                    "",
                    paragraph,
                    "",
                    f"- [fact] {text} #measurement",
                    f"- [fact] {shared} #measurement",
                    "",
                ]
                path = root / "pairs" / f"{slugify(side_title)}.md"
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("\n".join(body_lines), encoding="utf-8")
                ends.append((permalink, body_lines.index(f"- [fact] {text} #measurement") + 1, text))
            items.append(
                {
                    "id": len(items),
                    "kind": kind,
                    "type": ptype,
                    "lang": lang,
                    "domain": domain,
                    "a": ends[0][0],
                    "b": ends[1][0],
                    "a_line": ends[0][1],
                    "b_line": ends[1][1],
                    "a_text": ends[0][2],
                    "b_text": ends[1][2],
                }
            )
    (out / "contradictions.json").write_text(json.dumps(items, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return items


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=20260914)
    parser.add_argument("--domains", type=int, default=5)
    parser.add_argument("--engrams-per-domain", type=int, default=2000)
    parser.add_argument("--out", type=Path, default=Path("evals/scale/corpus"))
    parser.add_argument(
        "--contradictions", type=int, default=0,
        help="plant this many flipped pairs and as many hard negatives",
    )
    args = parser.parse_args()

    stats = generate(
        args.out.resolve(), args.seed, args.domains, args.engrams_per_domain
    )

    if args.contradictions:
        planted = plant_contradictions(
            args.out.resolve(), args.seed, args.contradictions
        )
        print(f"planted:      {len(planted)} pairs, sidecar contradictions.json")

    engrams = stats["engrams"]
    tokens = stats["body_tokens"]
    print(f"corpus:       {args.out}")
    print(f"seed:         {args.seed}")
    print(f"domains:      {stats['domains']}")
    print(f"engrams:      {engrams}")
    print(f"files:        {stats['files']} (engrams plus one MANIFEST per domain)")
    print(f"bytes:        {stats['bytes']} ({stats['bytes'] / 1048576:.1f} MiB)")
    print(f"body tokens:  {tokens} (chars/4, verify's own estimate)")
    if engrams:
        print(f"mean tokens:  {tokens / engrams:.0f} per engram")
        print(
            "distribution: "
            f"{stats['buckets'][0]} under 800, "
            f"{stats['buckets'][1]} to 2000, "
            f"{stats['buckets'][2]} to {TOKEN_BUDGET}, "
            f"{stats['buckets'][3]} over {TOKEN_BUDGET}"
        )
    print(f"over budget:  {stats['over_budget']} engrams trip the Q002 rule")
    print(f"links:        {stats['links']} ({stats['cross_domain_links']} cross-domain)")
    est_chunks = round(tokens / 450 * 1.25)
    print(f"chunks:       roughly {est_chunks} expected at a 450 token chunk budget")
    return 0


if __name__ == "__main__":
    sys.exit(main())
