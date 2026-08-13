//! Stage the built Fluid bundle where the embed macro can read it.
//!
//! `fluid/dist` is a build artifact: it is not in the tree, and a clone
//! without a node toolchain never grows one. The embed derive, on the other
//! hand, refuses to compile against a folder that does not exist. This script
//! is the seam between those two facts: it copies `fluid/dist` into
//! `$OUT_DIR/fluid-dist` when there is one and creates that folder empty when
//! there is not, so `#[folder = "$OUT_DIR/fluid-dist"]` always has something
//! to read. A binary built without a bundle then carries an empty embed, and
//! the serving layer answers 503 with the command that fixes it (see
//! `src/ui.rs`) rather than failing the build of everyone who only wants the
//! daemon.
//!
//! Staging through `$OUT_DIR` rather than pointing the derive straight at
//! `fluid/dist` is what buys that tolerance, and it keeps every write this
//! script makes inside cargo's own build directory instead of the source tree.
//!
//! ## The one trade-off, stated plainly
//!
//! `cargo:rerun-if-changed=../../fluid/dist` is emitted ONLY when the folder
//! exists. Cargo treats a watched path that is missing as always dirty, so an
//! unconditional line would rerun this script and rebuild the crate on every
//! single incremental build of a clone with no bundle - exactly the population
//! the graceful-empty behavior above exists to protect.
//!
//! What that costs: on a machine whose last cargo build saw no `fluid/dist`,
//! the FIRST `pnpm --dir fluid build` is not noticed on its own. Run
//! `touch crates/service/build.rs` (or `cargo clean -p crystalline-service`)
//! once after that first bundle build and every later bundle change is picked
//! up normally. The daemon's startup line about a binary with no web UI is
//! what makes the stale state visible instead of silent.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Where the bundle lands, relative to this crate's manifest.
const DIST: &str = "../../fluid/dist";

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("cargo always sets CARGO_MANIFEST_DIR"),
    );
    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("cargo always sets OUT_DIR for a build script"),
    );
    let staged = out_dir.join("fluid-dist");

    // Always start from an empty staging folder: a chunk whose name vanished
    // from a later bundle must not survive in the binary.
    if staged.exists() {
        fs::remove_dir_all(&staged)
            .unwrap_or_else(|e| panic!("clearing {}: {e}", staged.display()));
    }
    fs::create_dir_all(&staged).unwrap_or_else(|e| panic!("creating {}: {e}", staged.display()));

    // With the feature off nothing reads the staged folder, so skip the copy;
    // the folder itself still gets created above, which keeps this script free
    // of a branch the embed macro would have to agree with.
    if env::var_os("CARGO_FEATURE_FLUID_UI").is_none() {
        return;
    }

    let dist = manifest_dir.join(DIST);
    if !dist.is_dir() {
        return;
    }

    copy_tree(&dist, &staged);
    // Cargo walks a directory path here recursively, so this one line covers
    // every chunk. Read the trade-off note at the top before making it
    // unconditional.
    println!("cargo:rerun-if-changed={DIST}");
}

/// Copy `from` into `to`, recursively, creating directories as it goes.
fn copy_tree(from: &Path, to: &Path) {
    let entries = fs::read_dir(from).unwrap_or_else(|e| panic!("reading {}: {e}", from.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("reading an entry of {}: {e}", from.display()));
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            fs::create_dir_all(&target)
                .unwrap_or_else(|e| panic!("creating {}: {e}", target.display()));
            copy_tree(&source, &target);
        } else {
            fs::copy(&source, &target).unwrap_or_else(|e| {
                panic!("copying {} to {}: {e}", source.display(), target.display())
            });
        }
    }
}
