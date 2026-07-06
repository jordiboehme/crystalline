//! Build script for the `crystalline` binary: one job only.
//!
//! Windows gives a process's main thread a 1 MiB stack by default, where
//! Linux and macOS give 8 MiB. Unoptimized (debug profile) builds of this
//! crate allocate over 1 MiB of main-thread frame before doing anything at
//! all - debug codegen gives every temporary in the large clap command tree
//! and dispatch its own stack slot - so every debug invocation overflows on
//! Windows while release builds stay comfortably small. Raising the PE
//! stack reserve to 8 MiB gives the main thread the same budget on every
//! platform, for the binary and for this crate's test executables alike.
//! (Command futures are additionally built inside a dedicated deep-stack
//! thread; see `on_runtime_value` in main.rs.)

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && env == "msvc" {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
}
