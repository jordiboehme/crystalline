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
`--stage all|base|embed|daemon`, and `STATE_DIR` in the environment for the
state directory, which defaults to `/tmp/crystalline-scale` because a unix
socket path holds at most 103 bytes and a state directory under a deep
working copy overruns it.

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
