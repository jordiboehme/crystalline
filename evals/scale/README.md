# The scalability harness

Ten thousand engrams, five domains, on one machine. The field corpus that
prompted this harness is about that size with plenty of content in it, and
nothing else in the repository exercises the product anywhere near it.

Two scripts. `generate.py` writes the corpus, `run.sh` runs the product
against it and measures every step. Neither the corpus nor the run output is
committed (both are gitignored); the numbers live in a dated note under
`research/`.

## What the corpus models

Five domains of two thousand engrams each, twenty folders per domain, written
as markdown on disk exactly as a domain on disk looks: a MANIFEST.md with
Scope and When to Use, frontmatter carrying `type`, `title`, `permalink`,
`tags`, `status`, a pinned `recorded_at` and a `generated` stamp, prose
carrying `[[Title]]` links, two observation bullets and a relation bullet.

The parts that matter for the measurement:

- Body length follows the field distribution: 70 percent between 200 and 800
  tokens, 25 percent to 2,000, 4 percent to 2,500 and 1 percent over the
  budget, so verify's oversized rule fires on a realistic handful.
- Three links on average inside the domain, one engram in twenty also
  reaching into another domain, so link resolution has real work to do.
- Three to six tags per engram from a fixed vocabulary of two hundred.
- A few percent carry an elapsed validity window or a review date now past,
  so the temporal sweep has something to find.
- A seed drives all of it: the same seed produces the same corpus.

## Regenerating and rerunning

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cargo build --release

python3 evals/scale/generate.py --out evals/scale/corpus
bash evals/scale/run.sh --stage base
bash evals/scale/run.sh --stage embed
bash evals/scale/run.sh --stage daemon
```

`base` registers the five domains and measures sync, status, verify, the
search battery, evolve, doctor and a non-embedding `reindex --full`; it leaves
every chunk waiting to be embedded. `embed` is `reindex --full --embed` and a
second battery: it measures the bulk embedding pass and the peak resident size
that pass reaches, which is the ceiling this harness exists to watch, and it is
a stage of its own because it takes the longest by far. `daemon` then serves
the already-embedded index, runs the battery through the daemon and samples the
daemon's resident size while it works.

Run them in that order. It is load bearing, not a habit: `reindex --full` keeps
the embedding of every chunk whose text is unchanged, and the daemon drains any
backlog on its own, so an `embed` stage run after `daemon` finds nothing left to
embed and would report seconds where the pass takes an hour. It refuses in that
state rather than reporting a number that looks like a result.

Options: `--bin` (default `target/release/crystalline`), `--corpus`, `--out`,
`--stage all|base|embed|daemon|nli`, `--nli-profile full`,
`--nli-lift yes|no`, and `STATE_DIR` in the environment for the state
directory, which defaults to `/tmp/crystalline-scale` because a unix socket
path holds at most 103 bytes and a state directory under a deep working copy
overruns it. `DRAIN_LIMIT` in the environment raises the `nli` stage's
pre-flight ceiling for a long timing run (default 1800 seconds).
`NLI_SEED_MODELS` in the environment points at a directory already holding one
or more NLI checkpoints in hf-hub cache shape (`models--<org>--<name>`), so a
profile's own checkpoint is reused instead of downloaded the first time that
profile runs; without it, only the embedding model is seeded and the NLI
checkpoint still downloads once per profile, as it would on a real install.

## Contradiction scoring

`generate.py --contradictions N` plants N flipped pairs and N hard negatives in
a `probes` domain of their own (so they never compete with natural pairs for
the per-domain pair cap) and writes the sidecar
`contradictions.json` beside the corpus: flips by type (negation, number or
version, date, antonym, entity swap, quantity word), half English, half German
and one in ten mixed, and negatives that must stay quiet (compatible facts, the
same frame with a different scope, different periods with and without dates,
paraphrases and entailments), with German number and date formats.

`run.sh --stage nli --nli-profile full` scores the
corpus with that profile after the `embed` stage and dumps every stored row to
`<out>/nli-<profile>.json`; `evals/nli/evaluate.py` turns a dump and the sidecar
into the report the measurement note is written from. Precision runs on a
small probes corpus with the related line lowered and the pair cap lifted
(the default `--nli-lift yes`); drain time and resident size run on the full
corpus with the product's own caps (`--nli-lift no`), because the full corpus's
lead cosines are degenerate and a lifted cap there means weeks of CPU. A
pre-flight stops a stage whose projected drain exceeds `DRAIN_LIMIT`, and a
drain that does not finish anyway marks the dump `"drained": false` rather than
reporting partial numbers silently. Each profile gets its own model cache
under `<out>`, seeded with a clone of the embedding model (and, with
`NLI_SEED_MODELS` set, that profile's own checkpoint too).

## Isolation

The run never touches the caller's knowledge, index, config or daemon. HOME
is redirected under `--out`, which moves the config, data and cache
directories with it; the state directory is redirected separately for the
socket length; `--config` and `--db` are passed explicitly to every verb that
accepts them. The one thing shared on purpose is the embedding model cache
(`CRYSTALLINE_MODELS_DIR`), so the embed stage measures embedding rather than
a download.

## Output

`<out>/results.csv` is one line per step: stage, step, exit code, wall
seconds, peak resident megabytes. `<out>/daemon-rss.csv` is the daemon's
resident size over time with the embedding counts beside it.
`<out>/logs/<step>.log` holds each step's stdout and `<step>.time` its
`/usr/bin/time -l` report. A non-zero exit is a column, never a stop: verify
exits non-zero on this corpus by design.
