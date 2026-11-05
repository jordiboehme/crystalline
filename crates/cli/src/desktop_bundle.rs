//! `crystalline desktop-bundle`: stage a Claude Desktop extension from the
//! source in [`crystalline_core::desktop_extension`].
//!
//! Hidden, because only the release and CI call it. It writes the files a
//! bundle needs under a folder, and for the hollow bundle it can also pack
//! them into a `.mcpb` with the core zip writer.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use crystalline_core::desktop_extension::{Bundle, files, pack};

/// Which Claude Desktop bundle to stage.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum BundleKind {
    /// The Node launcher without a binary, built on the user's machine.
    Hollow,
    /// The released macOS bundle; the caller stages `server/crystalline`.
    DesktopOnly,
}

/// Write the files of `kind` under `outdir`, and with `pack_to` the same
/// files as a zip. `binary` is the path the hollow manifest pins.
pub fn run(
    kind: BundleKind,
    outdir: &Path,
    binary: Option<PathBuf>,
    pack_to: Option<PathBuf>,
) -> Result<()> {
    let hollow = matches!(kind, BundleKind::Hollow);
    if binary.is_some() && !hollow {
        bail!("--binary applies only to the hollow bundle.");
    }
    if pack_to.is_some() && !hollow {
        bail!(
            "--pack applies only to the hollow bundle: the release packs the Desktop-only bundle with its binary."
        );
    }
    let binary = binary.map(|path| path.to_string_lossy().into_owned());
    let bundle = match kind {
        BundleKind::Hollow => Bundle::Hollow {
            binary: binary.as_deref(),
        },
        BundleKind::DesktopOnly => Bundle::DesktopOnly,
    };
    let staged = files(bundle, env!("CARGO_PKG_VERSION"));
    for (path, bytes) in &staged {
        let target = outdir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        std::fs::write(&target, bytes)
            .with_context(|| format!("could not write {}", target.display()))?;
    }
    if let Some(file) = pack_to {
        let zip = pack(&staged).context("could not pack the bundle")?;
        std::fs::write(&file, zip)
            .with_context(|| format!("could not write {}", file.display()))?;
    }
    println!("wrote {}", outdir.join("manifest.json").display());
    Ok(())
}
