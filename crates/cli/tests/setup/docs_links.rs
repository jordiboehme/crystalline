//! The user docs must not link to a section that no longer exists.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

#[test]
fn no_link_to_shared_database_collaboration() {
    let root = repo_root();
    let mut files = vec![root.join("README.md")];
    for entry in std::fs::read_dir(root.join("docs")).expect("docs folder") {
        let path = entry.expect("docs entry").path();
        if path.extension().is_some_and(|e| e == "md") {
            files.push(path);
        }
    }
    assert!(files.len() > 2, "the scan found the docs: {files:?}");
    let mut hits = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read doc");
        if text.contains("#shared-database-collaboration") {
            hits.push(file.display().to_string());
        }
    }
    assert!(
        hits.is_empty(),
        "these files still link to the old section: {hits:?}"
    );
}

#[test]
fn the_postgres_section_exists_where_the_readme_links() {
    let root = repo_root();
    let deployment = std::fs::read_to_string(root.join("docs/deployment.md")).expect("deployment");
    assert!(deployment.contains("\n## Run on PostgreSQL\n"));
    let readme = std::fs::read_to_string(root.join("README.md")).expect("readme");
    assert!(readme.contains("docs/deployment.md#run-on-postgresql"));
}
