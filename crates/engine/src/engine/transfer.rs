use super::*;

impl Engine {
    // --- domain import / export / scaffold -----------------------------------

    /// Scaffold a MANIFEST engram into a virtual domain from prebuilt markdown,
    /// unless one already exists. A no-op that reports `created: false` when the
    /// domain already has a `MANIFEST.md`. Refuses on a file domain (its MANIFEST
    /// belongs on disk via `domain init`).
    pub async fn scaffold_virtual_manifest(&self, domain: &str, markdown: &str) -> Result<Value> {
        if let ContentSource::File { .. } = self.content_source(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain '{domain}' is a file domain; scaffold its MANIFEST on disk with `crystalline domain init`"
            )));
        }
        let store = self.store.lock().await;
        let domain_id = store
            .upsert_domain(domain, None, DomainKind::Virtual)
            .await?;
        let existing = store.engram_content(domain_id, "MANIFEST.md").await?;
        drop(store);
        if existing.is_some() {
            return Ok(json!({ "domain": domain, "manifest": "MANIFEST.md", "created": false }));
        }
        let stamp = virtual_stamp(markdown);
        let store = self.store.lock().await;
        self.index_markdown(
            &*store,
            domain_id,
            "MANIFEST.md",
            markdown,
            stamp,
            None,
            true,
        )
        .await?;
        drop(store);

        // The MANIFEST engram just landed; its Scope and When to Use bullets are
        // exactly what the routing block reads for this virtual domain, so
        // refresh the cache the sync `routing_text` serves.
        self.refresh_routing_cache().await;

        Ok(json!({ "domain": domain, "manifest": "MANIFEST.md", "created": true }))
    }

    /// Import already-well-formed engram `.md` files from `src` into a virtual
    /// domain verbatim. Refuses a file target (that would desync the DB from its
    /// files). Collisions on an existing path or permalink are skipped unless
    /// `overwrite`; `dry_run` reports without writing.
    pub async fn import_domain(
        &self,
        domain: &str,
        src: &Path,
        overwrite: bool,
        dry_run: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let ContentSource::File { .. } = self.content_source(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain import loads into a virtual domain; '{domain}' is a file domain. \
                 Use `crystalline import` then `crystalline sync` for a file domain."
            )));
        }
        if !src.is_dir() {
            return Err(EngineError::Invalid(format!(
                "import source '{}' is not a directory",
                src.display()
            )));
        }

        let files = walk_markdown(src);
        let store = self.store.lock().await;
        let domain_id = store
            .upsert_domain(domain, None, DomainKind::Virtual)
            .await?;
        let existing = store.all_engram_contents(domain_id).await?;
        drop(store);
        let existing_paths: HashSet<String> = existing.iter().map(|e| e.path.clone()).collect();
        let existing_perms: HashSet<String> =
            existing.iter().map(|e| e.permalink.clone()).collect();

        let mut written = 0usize;
        let mut skipped = 0usize;
        let mut collisions: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut changes: Vec<Value> = Vec::new();

        for (rel, abs) in files {
            let text = match std::fs::read_to_string(&abs) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("{rel}: could not read: {e}"));
                    continue;
                }
            };
            let engram = match parse_engram(&text) {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("{rel}: could not parse: {e}"));
                    continue;
                }
            };
            let record = EngramRecord::from_engram(&engram, &rel, virtual_stamp(&text));
            let collides = (existing_paths.contains(&rel)
                || existing_perms.contains(&record.permalink))
                && !overwrite;
            if collides {
                collisions.push(rel.clone());
                skipped += 1;
                continue;
            }
            if dry_run {
                changes.push(json!({ "path": rel, "permalink": record.permalink }));
                written += 1;
                continue;
            }
            let stamp = virtual_stamp(&text);
            let store = self.store.lock().await;
            match self
                .index_markdown(&*store, domain_id, &rel, &text, stamp, None, true)
                .await
            {
                Ok(_) => {
                    changes.push(json!({ "path": rel, "permalink": record.permalink }));
                    written += 1;
                }
                Err(e) => {
                    warnings.push(format!("{rel}: {e}"));
                    skipped += 1;
                }
            }
        }

        Ok(json!({
            "domain": domain,
            "dry_run": dry_run,
            "files_written": written,
            "files_skipped": skipped,
            "collisions": collisions,
            "warnings": warnings,
            "files": changes,
        }))
    }

    /// Import in-memory engram files - an unpacked archive - into a domain of
    /// either storage kind, classifying every entry as `create`, `overwrite`,
    /// `skip`, `invalid` or `ignored`.
    ///
    /// One verb backs both the preview and the commit: `dry_run` runs the whole
    /// classification and writes nothing, so what an operator is shown is what
    /// the same call would then do. Each kind is written through its own normal
    /// road rather than a shortcut: a file domain gets the exact incoming bytes
    /// on disk and is indexed by one targeted [`Engine::sync_paths`] after the
    /// loop, the same pass the watcher runs for an external edit, and a virtual
    /// domain is indexed directly because the row is its only source of truth.
    /// Two files must never claim one permalink, so a permalink already held at
    /// a different path is refused under both policies - `overwrite` decides
    /// what happens at the SAME path, nothing more.
    pub async fn import_domain_files(
        &self,
        domain: &str,
        files: &[(String, String)],
        overwrite: bool,
        dry_run: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // An import writes the folder the team shares under an admin's hand,
        // and a domain that reviews changes has no answer for whose draft that
        // would be. The preview is exempt because it writes nothing: a question
        // about a refusal is not the refusal.
        if !dry_run {
            self.refuse_write_into_reviewed_folder(
                domain,
                "an imported archive cannot land there",
            )?;
        }
        // Delta 2 vs `import_domain`: a file domain is served too, so the source
        // decides how a write lands rather than being refused outright.
        let source = self.content_source(domain)?;
        let store = self.store.lock().await;
        let domain_id = match &source {
            ContentSource::File { root } => {
                store
                    .upsert_domain(domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?
            }
            ContentSource::Virtual => {
                store
                    .upsert_domain(domain, None, DomainKind::Virtual)
                    .await?
            }
        };
        let existing = store.all_engram_contents(domain_id).await?;
        drop(store);

        // The snapshot is taken once, before the loop, and then kept live as the
        // batch is classified: an entry accepted here claims its path and its
        // permalink for the rest of the batch, so two files of one archive
        // claiming one permalink are resolved deterministically in input order
        // (first wins, second skips) instead of both passing a stale snapshot.
        let mut path_perms: HashMap<String, String> = HashMap::new();
        let mut perm_paths: HashMap<String, String> = HashMap::new();
        for e in &existing {
            path_perms.insert(e.path.clone(), e.permalink.clone());
            perm_paths.insert(e.permalink.clone(), e.path.clone());
        }

        let mut created = 0usize;
        let mut overwritten = 0usize;
        let mut skipped = 0usize;
        let mut invalid = 0usize;
        let mut ignored = 0usize;
        // Delta 6: every entry gets a row, whatever became of it.
        let mut entries: Vec<Value> = Vec::new();
        let mut changed_paths: Vec<String> = Vec::new();

        // Delta 1: the files arrive in memory, already unpacked by the caller,
        // so there is no folder to walk and no source directory to validate.
        for (path, text) in files {
            // Delta 4: a MANIFEST at any depth is ignored - defense in depth,
            // the REST layer screens these before the engine ever sees them.
            // Matched case-insensitively because the filesystem underneath is:
            // on APFS or NTFS a `manifest.md` entry lands on the domain's real
            // MANIFEST.md, so an exact-string screen would let a third-party
            // archive replace the one file a domain cannot regenerate.
            if Path::new(path)
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("MANIFEST.md"))
            {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "ignored",
                    "reason": "a MANIFEST belongs to the domain, which keeps its own",
                }));
                ignored += 1;
                continue;
            }
            // Only markdown is an engram. Anything else would land in the folder
            // as junk a sync never walks, or as a row no reader can parse, so it
            // is reported rather than written.
            if !path.to_lowercase().ends_with(".md") {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "ignored",
                    "reason": "not a markdown engram file",
                }));
                ignored += 1;
                continue;
            }
            // The OKF reserved names are never imported: `index.md` is rebuilt
            // from the folder it sits in, and `log.md` is reserved without ever
            // being generated at all, so an import can only damage it.
            //
            // Matched case-insensitively, and deliberately stricter than
            // `crystalline_core::is_reserved_path` (whose exact match is a
            // documented rule about what Crystalline generates and exports).
            // Import faces the filesystem instead, and that filesystem is
            // case-insensitive on APFS and NTFS: a `Log.md` entry renames onto
            // the existing `log.md`, replacing its bytes while the on-disk name
            // stays lowercase. Nothing regenerates a log, so that loss is
            // permanent - the same argument that makes the MANIFEST screen
            // above case-insensitive.
            if Path::new(path).file_name().is_some_and(|name| {
                name.eq_ignore_ascii_case(crystalline_core::INDEX_FILE)
                    || name.eq_ignore_ascii_case(crystalline_core::LOG_FILE)
            }) {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "ignored",
                    "reason": "a reserved OKF index or log is never imported",
                }));
                ignored += 1;
                continue;
            }
            // An archive is untrusted input and `join_rel` joins segment by
            // segment, `..` included, so containment is decided before any path
            // is built: an entry can never address a byte outside the domain.
            if !is_contained_rel(path) {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "invalid",
                    "reason": "path escapes the domain root",
                }));
                invalid += 1;
                continue;
            }
            // Delta 5: unparseable content is a first-class `invalid` entry
            // carrying the parse error, not a warning on the side.
            let engram = match parse_engram(text) {
                Ok(e) => e,
                Err(e) => {
                    entries.push(json!({
                        "path": path,
                        "permalink": Value::Null,
                        "action": "invalid",
                        "reason": e.to_string(),
                    }));
                    invalid += 1;
                    continue;
                }
            };
            // `parse_engram` is deliberately permissive - a file with no
            // frontmatter at all parses into an engram with empty fields - so
            // the required OKF keys are checked here. Without them there is
            // nothing to import: the permalink would be invented from the file
            // name and the engram would carry no type.
            if engram.frontmatter.engram_type.trim().is_empty()
                || engram.frontmatter.title.trim().is_empty()
            {
                entries.push(json!({
                    "path": path,
                    "permalink": Value::Null,
                    "action": "invalid",
                    "reason": "not an engram: the frontmatter needs a type and a title",
                }));
                invalid += 1;
                continue;
            }
            let record = EngramRecord::from_engram(&engram, path, virtual_stamp(text));

            // Delta 3: a permalink held at a different path is refused under
            // BOTH policies - `overwrite` is a same-path decision only.
            if let Some(held_at) = perm_paths.get(&record.permalink)
                && held_at != path
            {
                entries.push(json!({
                    "path": path,
                    "permalink": record.permalink,
                    "action": "skip",
                    "reason": format!(
                        "permalink '{}' already exists at another path",
                        record.permalink
                    ),
                }));
                skipped += 1;
                continue;
            }
            // A file domain's truth is the file on disk, so an entry that never
            // made it into the index still counts as existing there.
            let exists = path_perms.contains_key(path)
                || match &source {
                    ContentSource::File { root } => join_rel(root, path).exists(),
                    ContentSource::Virtual => false,
                };
            if exists && !overwrite {
                entries.push(json!({
                    "path": path,
                    "permalink": record.permalink,
                    "action": "skip",
                    "reason": format!("'{path}' already exists"),
                }));
                skipped += 1;
                continue;
            }

            if !dry_run {
                let outcome = match &source {
                    // Delta 2: the exact incoming bytes go to disk and the index
                    // follows afterwards through the targeted sync, so an import
                    // takes the same road as any external write.
                    ContentSource::File { root } => {
                        write_file(&join_rel(root, path), text).map(|()| {
                            changed_paths.push(path.clone());
                        })
                    }
                    // A virtual domain has no file: the row is the document, so
                    // the full markdown is indexed directly.
                    ContentSource::Virtual => {
                        let stamp = virtual_stamp(text);
                        let store = self.store.lock().await;
                        self.index_markdown(&*store, domain_id, path, text, stamp, None, true)
                            .await
                            .map(|_| ())
                    }
                };
                if let Err(e) = outcome {
                    entries.push(json!({
                        "path": path,
                        "permalink": record.permalink,
                        "action": "skip",
                        "reason": e.to_string(),
                    }));
                    skipped += 1;
                    continue;
                }
            }

            if exists {
                overwritten += 1;
            } else {
                created += 1;
            }
            entries.push(json!({
                "path": path,
                "permalink": record.permalink,
                "action": if exists { "overwrite" } else { "create" },
                "reason": Value::Null,
            }));
            // Claim both for the rest of the batch. An overwrite that changes an
            // engram's permalink releases the one that path used to hold, so a
            // later entry is judged against what the batch will really leave
            // behind - in a dry run too, where the preview must match the commit.
            if let Some(prev) = path_perms.insert(path.clone(), record.permalink.clone())
                && prev != record.permalink
            {
                perm_paths.remove(&prev);
            }
            perm_paths.insert(record.permalink, path.clone());
        }

        // Delta 2, after the loop and only once: one targeted sync for the whole
        // batch, the pass the watcher runs for a small debounced set of paths,
        // rather than a full rescan or a per-file reindex. A dry run collected
        // no path here, so it never reaches the sync either.
        if !changed_paths.is_empty() {
            self.sync_paths(domain, changed_paths).await?;
        }

        Ok(json!({
            "domain": domain,
            "dry_run": dry_run,
            "files": entries,
            "created": created,
            "overwritten": overwritten,
            "skipped": skipped,
            "invalid": invalid,
            "ignored": ignored,
        }))
    }

    /// Every file of a domain as `(domain-relative path, bytes)`, MANIFEST and
    /// attachments included: the portable view an archive download is built
    /// from, byte for byte as the domain holds it.
    ///
    /// Bytes rather than text, and that is the whole reason for the type: an
    /// attachment is a PNG or a slide deck, and a collection of `String` could
    /// only carry a domain's markdown. Markdown entries are the same bytes they
    /// always were - UTF-8 is validated where a body is parsed, not here, since
    /// nothing on this path parses anything.
    ///
    /// Each storage kind is read from its own source of truth, which is why
    /// this is not simply `export_domain`'s read half. A file domain's truth is
    /// the markdown on disk, walked exactly the way a sync walks it: the index
    /// keeps only the body there, with the frontmatter shredded into columns,
    /// so reading the store would hand back headerless engrams and a MANIFEST
    /// that never indexed would go missing entirely. A virtual domain has no
    /// disk at all - the row IS the file, and it carries the full text.
    ///
    /// Attachments come last, and through the seam rather than off either
    /// source directly ([`Engine::attachment_list`] then
    /// [`Engine::attachment_read`]), so both kinds hand over the same bytes
    /// under the same `assets/` paths - which is what lets an export of one
    /// kind be imported as the other. An attachment whose row stands but whose
    /// bytes cannot be read (a file deleted behind the index) is logged and
    /// skipped, like an unreadable markdown file: a backup missing one file
    /// beats no backup.
    pub async fn domain_files(&self, domain: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let entry = self.domain_entry(domain)?;
        let mut files = match self.source_of(&entry) {
            ContentSource::File { root } => {
                let mut files = Vec::new();
                for (rel, abs) in walk_markdown(&root) {
                    // The OKF reserved names are excluded, as everywhere else:
                    // `index.md` is generated from the folder it sits in and
                    // regenerates itself wherever this archive is restored,
                    // and writing either name back is refused by design.
                    if crystalline_core::is_reserved_path(&rel) {
                        continue;
                    }
                    match std::fs::read(&abs) {
                        Ok(bytes) => files.push((rel, bytes)),
                        // One unreadable file must not deny the operator the
                        // rest of the backup: it is skipped and logged rather
                        // than failing the whole archive.
                        Err(e) => {
                            tracing::warn!("archive of '{domain}' skipped '{rel}': {e}");
                        }
                    }
                }
                files
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(domain, None, DomainKind::Virtual)
                    .await?;
                let all = store.all_engram_contents(domain_id).await?;
                drop(store);
                all.into_iter()
                    .map(|e| (e.path, e.content.into_bytes()))
                    .collect()
            }
        };
        for row in self.attachment_list(domain).await? {
            match self.attachment_read(domain, &row.path).await {
                Ok((bytes, _)) => files.push((row.path, bytes)),
                Err(e) => {
                    tracing::warn!("archive of '{domain}' skipped '{}': {e}", row.path);
                }
            }
        }
        Ok(files)
    }

    /// Export every file of a domain (file or virtual) to `dest` as a normal
    /// filesystem engram folder. Refuses to write into a non-empty directory
    /// unless `force`; `dry_run` reports without writing.
    ///
    /// The read half is [`Engine::domain_files`], the same one the archive
    /// download uses, so an export is a copy of the domain rather than a
    /// re-serialization of the index: a file domain hands over its exact disk
    /// bytes (frontmatter included, MANIFEST included), a virtual domain the
    /// full text of every row, both hand over their attachments under
    /// `assets/`, and the OKF reserved names are excluded from both. Reading
    /// the store directly instead - the shape this verb had - wrote
    /// frontmatter-less markdown for file domains, since their index rows keep
    /// only the body, and silently dropped MANIFEST.md.
    ///
    /// Report shape follows from that source: `domain_files` carries
    /// `(path, bytes)` and no permalink column, so each row reports its
    /// path and byte count instead of the former path/permalink pair. Parsing
    /// every body back just to re-derive a permalink would re-introduce the
    /// re-serialization this verb exists to avoid, and no caller reads the
    /// field: the two callers (`ctl` and the daemonless CLI) print the report
    /// as-is.
    pub async fn export_domain(
        &self,
        domain: &str,
        dest: &Path,
        force: bool,
        dry_run: bool,
    ) -> Result<Value> {
        let all = self.domain_files(domain).await?;

        if !dry_run && dir_is_nonempty(dest) && !force {
            return Err(EngineError::Conflict(format!(
                "destination '{}' is not empty; pass force to overwrite",
                dest.display()
            )));
        }

        let mut written = 0usize;
        let mut files: Vec<Value> = Vec::new();
        for (path, content) in &all {
            files.push(json!({ "path": path, "bytes": content.len() }));
            if dry_run {
                continue;
            }
            let abs = join_rel(dest, path);
            write_bytes(&abs, content)?;
            written += 1;
        }

        Ok(json!({
            "domain": domain,
            "dest": dest.display().to_string(),
            "dry_run": dry_run,
            "files_written": if dry_run { all.len() } else { written },
            "files": files,
        }))
    }
}
