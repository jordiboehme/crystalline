#!/usr/bin/env python3
"""Write the NLI parity fixtures the Rust loader is checked against.

For every model in the contradiction check's table (or the one --model names)
this loads the checkpoint with Python transformers in float32, scores 21
fixed pairs (English, German and mixed, one German line cut at 254 tokens),
one pair per forward pass with no
padding, after cutting each line the way the Rust loader does (first to
254 x 16 characters, then to 254 tokens), and writes the token ids and the raw
logits to crates/index/tests/fixtures/nli-parity-<id>.json together with the
transformers and torch versions it ran with.

    uv venv --python 3.12 .venv
    uv pip install --python .venv/bin/python --index-url https://download.pytorch.org/whl/cpu torch
    uv pip install --python .venv/bin/python transformers sentencepiece protobuf tiktoken
    .venv/bin/python evals/nli/parity.py
    .venv/bin/python evals/nli/parity.py --model MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli

Point HF_HUB_CACHE at the directory the Rust loader downloaded into
(CRYSTALLINE_MODELS_DIR) and set HF_HUB_OFFLINE=1 to check the very bytes the
Rust side loads, with no second download.
"""

import argparse
import datetime
import json
import sys
from pathlib import Path

import torch
import transformers
from transformers import AutoModelForSequenceClassification, AutoTokenizer

# repo -> the short id the Rust table names it by (crates/index/src/nli/models.rs)
MODELS = {
    "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7": "mdeberta-v3-base-xnli-2mil7",
    "MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli": "multilingual-minilmv2-l12-mnli-xnli",
    "MoritzLaurer/DeBERTa-v3-base-mnli-fever-anli": "deberta-v3-base-mnli-fever-anli",
}

MAX_LINE_TOKENS = 254
# The character cut before tokenizing, as MAX_LINE_CHARS in crates/index/src/nli/local.rs.
MAX_LINE_CHARS = MAX_LINE_TOKENS * 16
TOLERANCE = 1e-4

PAIRS = [
    ("en-version", "The build uses Node 18", "The build uses Node 20"),
    ("en-unrelated", "The build uses Node 18", "Deployments run on Fridays"),
    ("en-negation", "The nightly backup runs on a fixed schedule", "The nightly backup does not run on a fixed schedule"),
    ("en-antonym", "The cache is enabled by default", "The cache is disabled by default"),
    ("en-entity", "The ingest service is owned by the platform team", "The ingest service is owned by the data team"),
    ("en-quantity", "Releases always need a manual approval", "Releases never need a manual approval"),
    ("en-entailment", "The job runs every night at two", "The job runs every night"),
    ("en-long", "The retry queue doubles its backoff on every failure and a dead-letter ttl bounds how long any single retry waits before the message is parked for a person to look at", "The retry queue keeps a constant backoff"),
    ("de-version", "Der Build nutzt Node 18", "Der Build nutzt Node 20"),
    ("de-unrelated", "Der Build nutzt Node 18", "Die Auslieferung ist freitags"),
    ("de-negation", "Der Server braucht keinen Neustart", "Der Server braucht einen Neustart"),
    ("de-number", "Der Puffer fasst 1.000 Einträge", "Der Puffer fasst 1.500 Einträge"),
    ("de-decimal", "Die Toleranz liegt bei 1,5 mm", "Die Toleranz liegt bei 2,5 mm"),
    ("de-date", "Die Abnahme war am 27.09.2026", "Die Abnahme war am 14.03.2025"),
    ("de-no-longer", "Der Nachtlauf nutzt nicht mehr das alte Skript", "Der Nachtlauf nutzt das alte Skript"),
    ("de-compound", "Die Kühlkettenüberwachung meldet stündlich", "Die Kühlkettenüberwachung meldet täglich"),
    ("mixed-version", "The build uses Node 18", "Der Build nutzt Node 20"),
    ("mixed-same", "The cache is enabled by default", "Der Cache ist standardmäßig aktiviert"),
    ("mixed-negation", "Der Server braucht keinen Neustart", "The server needs a restart"),
    ("mixed-unrelated", "Die Toleranz liegt bei 1,5 mm", "Deployments run on Fridays"),
    # Far past 254 tokens with umlauts throughout, so the token cut is under
    # the gate and Rust's byte offsets and Python's char offsets differ at it.
    (
        "de-long-cut",
        " ".join(["Die Kühlkettenüberwachung meldet stündlich größere Störungen an die Leitstelle."] * 30),
        "Die Kühlkettenüberwachung meldet täglich",
    ),
]


def cap(tokenizer, text: str) -> str:
    """Cut `text` to MAX_LINE_CHARS characters, then after its 254th token, as the Rust loader's cap_line does."""
    text = text[:MAX_LINE_CHARS]
    enc = tokenizer(text, add_special_tokens=False, return_offsets_mapping=True)
    if len(enc["input_ids"]) <= MAX_LINE_TOKENS:
        return text
    return text[: enc["offset_mapping"][MAX_LINE_TOKENS - 1][1]]


def fixture_for(repo: str) -> dict:
    tokenizer = AutoTokenizer.from_pretrained(repo)
    # The offsets `cap` reads exist only on a fast (tokenizer.json) tokenizer,
    # which is also the file the Rust loader reads.
    assert tokenizer.is_fast, f"{repo}: not a fast tokenizer"
    # transformers 5 loads the checkpoint's own dtype by default, and two of
    # the three are float16 on disk: the reference must run in float32.
    model = AutoModelForSequenceClassification.from_pretrained(repo, dtype=torch.float32)
    model.eval()
    dtypes = {p.dtype for p in model.parameters()}
    assert dtypes == {torch.float32}, f"{repo}: parameters in {dtypes}"
    items = []
    for label, premise, hypothesis in PAIRS:
        enc = tokenizer(
            cap(tokenizer, premise),
            cap(tokenizer, hypothesis),
            return_tensors="pt",
            truncation=True,
            max_length=512,
        )
        with torch.no_grad():
            logits = model(**enc).logits[0].tolist()
        items.append(
            {
                "label": label,
                "premise": premise,
                "hypothesis": hypothesis,
                "tokens": int(enc["input_ids"].shape[1]),
                "input_ids": enc["input_ids"][0].tolist(),
                "logits": logits,
            }
        )
    return {
        "model": repo,
        "reference": (
            f"transformers {transformers.__version__}, torch {torch.__version__}, float32, "
            f"fast tokenizer, one pair per forward, {datetime.date.today().isoformat()}"
        ),
        "transformers": transformers.__version__,
        "torch": torch.__version__,
        "dtype": "float32",
        "fast_tokenizer": True,
        "id2label": {str(k): v for k, v in model.config.id2label.items()},
        "tolerance": TOLERANCE,
        "pairs": items,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--model", choices=sorted(MODELS), help="one repository; default every model")
    parser.add_argument("--out", type=Path, default=Path("crates/index/tests/fixtures"))
    args = parser.parse_args()
    repos = [args.model] if args.model else list(MODELS)
    args.out.mkdir(parents=True, exist_ok=True)
    for repo in repos:
        path = args.out / f"nli-parity-{MODELS[repo]}.json"
        path.write_text(json.dumps(fixture_for(repo), indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
