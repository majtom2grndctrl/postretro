// Source-derived upload ownership and staging-mechanism gates.
// See: context/plans/in-progress/per-frame-upload-batching, AC 2/7/18

mod scanner;
use scanner::*;
use std::path::Path;

#[test]
fn renderer_direct_uploads_and_submissions_have_lifecycle_owners() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = load_sources(&root);
    let (sites, _) = scan(&sources);
    assert!(
        !sites.is_empty(),
        "module scan must reach renderer upload sites"
    );
    for site in &sites {
        if site.method != "submit"
            && (site.receiver == Receiver::RawQueue || site.method == "direct_write_buffer")
        {
            println!(
                "{}: {}",
                direct_class(site).unwrap_or("UNCLASSIFIED"),
                site.location()
            );
        }
    }
    let errors = violations(&sites);
    assert!(
        errors.is_empty(),
        "upload ownership drift:\n{}",
        errors.join("\n")
    );
}

#[test]
fn renderer_uses_only_the_shared_staging_pool() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let (_, errors) = scan(&load_sources(&root));
    assert!(
        errors.is_empty(),
        "upload mechanism drift:\n{}",
        errors.join("\n")
    );
}

fn fixture(source: &str) -> Vec<Source> {
    vec![Source {
        path: "fixture.rs".into(),
        syntax: syn::parse_file(source).unwrap(),
    }]
}

#[test]
fn drift_scanner_distinguishes_typed_receivers_fields_aliases_and_raw_escape() {
    let (sites, _) = scan(&fixture(
        r#"
        struct Renderer { queue: UploadQueue }
        impl Renderer {
            fn frame(&self, staging: &mut StagedUploads) {
                let Self { queue, .. } = self;
                queue.write_buffer(target, 0, bytes);
                self.queue.submit(commands);
                staging.write_buffer(target, 0, bytes);
                let direct = self.queue.raw();
                direct.write_buffer(target, 0, bytes);
            }
        }
        fn raw(queue: &wgpu::Queue) { queue.submit(commands); }
    "#,
    ));
    assert_eq!(
        sites.iter().map(|site| &site.receiver).collect::<Vec<_>>(),
        vec![
            &Receiver::UploadQueue,
            &Receiver::UploadQueue,
            &Receiver::StagedUploads,
            &Receiver::RawQueue,
            &Receiver::RawQueue,
        ]
    );
    assert_eq!(
        violations(&sites).len(),
        2,
        "both raw escapes must fail while staged calls pass"
    );
}

#[test]
fn drift_scanner_skips_test_items_and_ignores_comments_and_strings() {
    let (sites, mechanisms) = scan(&fixture(
        r##"
        // raw.write_buffer(..); wgpu::util::StagingBelt
        const TEXT: &str = r#"queue.submit(); BufferUsages::MAP_WRITE"#;
        #[cfg(test)] mod tests { fn hidden(q: &wgpu::Queue) { q.submit(commands); } }
        #[cfg(all(test, feature = "dev-tools"))]
        fn hidden(q: &wgpu::Queue) { q.write_buffer(target, 0, bytes); }
        impl Renderer {
            #[cfg(test)] fn hidden(&self) { self.raw.submit(commands); }
        }
        fn expression_gate(q: &wgpu::Queue) {
            #[cfg(test)] { q.submit(commands); }
        }
        #[cfg(any(test, feature = "dev-tools"))]
        fn production(q: &wgpu::Queue) { q.write_buffer(target, 0, bytes); }
    "##,
    ));
    assert_eq!(
        sites.len(),
        1,
        "a feature-enabled production branch must still be scanned"
    );
    assert_eq!(sites[0].function, "production");
    assert!(mechanisms.is_empty());
}

#[test]
fn drift_scanner_follows_production_modules_and_excludes_test_only_files() {
    let root = std::env::temp_dir().join(format!(
        "postretro-upload-module-gate-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(
        root.join("lib.rs"),
        "mod nested; #[cfg(test)] mod missing_test_file;",
    )
    .unwrap();
    std::fs::write(
        root.join("nested.rs"),
        "mod leaf; #[cfg(test)] mod missing_test_file;",
    )
    .unwrap();
    std::fs::write(
        root.join("nested/leaf.rs"),
        "fn frame(queue: &wgpu::Queue) { queue.submit(commands); }",
    )
    .unwrap();
    let sources = load_sources(&root);
    std::fs::remove_dir_all(&root).unwrap();
    let (sites, _) = scan(&sources);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].path, "nested/leaf.rs");
    assert_eq!(violations(&sites).len(), 1);
}

#[test]
fn drift_scanner_rejects_ufcs_and_macro_upload_escapes() {
    let (sites, mechanisms) = scan(&fixture(
        r#"
        fn raw(queue: &wgpu::Queue) {
            wgpu::Queue::write_buffer(queue, target, 0, bytes);
            wgpu::Queue::submit(queue, commands);
            opaque_macro!(queue.write_buffer(target, 0, bytes));
            opaque_macro!(wgpu::BufferUsages::MAP_WRITE);
        }
    "#,
    ));
    assert_eq!(violations(&sites).len(), 3);
    assert_eq!(mechanisms.len(), 1);
}

#[test]
fn drift_scanner_rejects_new_writers_unresolved_receivers_and_staging_mechanisms() {
    let (sites, mechanisms) = scan(&fixture(
        r#"
        fn frame(q: &wgpu::Queue) { q.write_buffer(target, 0, bytes); }
        fn untyped() { untracked.write_texture(target, bytes, layout, size); }
        fn staging() {
            let belt = wgpu::util::StagingBelt::new(1024);
            let usage = wgpu::BufferUsages::MAP_WRITE;
        }
    "#,
    ));
    assert_eq!(violations(&sites).len(), 2);
    assert_eq!(mechanisms.len(), 2);
}

// Regression: shadowing closure arguments inherited the outer staged queue type.
#[test]
fn drift_scanner_scopes_typed_and_untyped_closure_arguments() {
    let (sites, _) = scan(&fixture(
        r#"
        fn frame(queue: &UploadQueue) {
            let raw = |queue: &wgpu::Queue| {
                queue.write_buffer(target, 0, bytes);
                let nested = |queue: &mut StagedUploads| {
                    queue.write_buffer(target, 0, bytes);
                };
                queue.submit(commands);
            };
            let untyped = |queue| {
                let alias = queue;
                alias.write_buffer(target, 0, bytes);
                queue.submit(commands);
            };
            raw(queue.raw());
            untyped(queue.raw());
            queue.write_buffer(target, 0, bytes);
            queue.submit(commands);
        }
    "#,
    ));
    assert_eq!(
        sites.iter().map(|site| &site.receiver).collect::<Vec<_>>(),
        vec![
            &Receiver::RawQueue,
            &Receiver::StagedUploads,
            &Receiver::RawQueue,
            &Receiver::Unknown,
            &Receiver::Unknown,
            &Receiver::UploadQueue,
            &Receiver::UploadQueue,
        ]
    );
    assert_eq!(violations(&sites).len(), 4);
}

#[test]
fn drift_scanner_resolves_shadowed_closure_fields_and_aliases() {
    let (sites, _) = scan(&fixture(
        r#"
        struct Staged { queue: UploadQueue }
        struct Direct { queue: wgpu::Queue }
        fn frame(holder: &Staged) {
            let raw = |holder: &Direct| {
                let alias = &holder.queue;
                alias.write_buffer(target, 0, bytes);
                holder.queue.submit(commands);
            };
            let staged = |holder: &Staged| holder.queue.submit(commands);
            holder.queue.write_buffer(target, 0, bytes);
        }
    "#,
    ));
    assert_eq!(
        sites.iter().map(|site| &site.receiver).collect::<Vec<_>>(),
        vec![
            &Receiver::RawQueue,
            &Receiver::RawQueue,
            &Receiver::UploadQueue,
            &Receiver::UploadQueue,
        ]
    );
    assert_eq!(violations(&sites).len(), 2);
}

// Regression: untyped pattern shadows hid raw operations behind an outer UploadQueue.
#[test]
fn drift_scanner_rejects_pattern_shadows_and_restores_outer_scope() {
    for branch in [
        "match Some(queue.raw()) { Some(queue) => { WRITE; SUBMIT; }, _ => {} }",
        "match Some(queue.raw()) { Some(queue) if { SUBMIT; true } => { WRITE; }, _ => {} }",
        "for queue in [queue.raw()] { WRITE; SUBMIT; }",
        "for (queue, _) in [(queue.raw(), ())] { WRITE; SUBMIT; }",
        "if let Some(queue) = Some(queue.raw()) { WRITE; SUBMIT; }",
        "while let Some(queue) = Some(queue.raw()) { WRITE; SUBMIT; break; }",
        "match (queue.raw(), ()) { (queue, _) => { WRITE; SUBMIT; } }",
        "match [queue.raw()] { [queue] => { WRITE; SUBMIT; } }",
        "match Ok(queue.raw()) { Ok(queue) | Err(queue) => { WRITE; SUBMIT; } }",
        "match queue.raw() { queue @ _ => { WRITE; SUBMIT; } }",
        "match holder { Holder { queue } => { WRITE; SUBMIT; } }",
    ] {
        let branch = branch
            .replace("WRITE", "queue.write_buffer(target, 0, bytes)")
            .replace("SUBMIT", "queue.submit(commands)");
        let source = format!(
            "fn frame(queue: &UploadQueue) {{ {branch} queue.write_buffer(target, 0, bytes); }}"
        );
        let (sites, _) = scan(&fixture(&source));
        assert_eq!(sites.len(), 3, "{branch}");
        assert_eq!(sites[0].receiver, Receiver::Unknown, "{branch}");
        assert_eq!(sites[1].receiver, Receiver::Unknown, "{branch}");
        assert_eq!(sites[2].receiver, Receiver::UploadQueue, "{branch}");
        assert_eq!(violations(&sites).len(), 2, "{branch}");
    }
}

#[test]
fn drift_scanner_restores_staged_receivers_between_match_arms_and_if_branches() {
    let (sites, _) = scan(&fixture(
        r#"
        fn frame(queue: &UploadQueue) {
            match Some(queue.raw()) {
                Some(queue) => queue.submit(commands),
                None => queue.submit(commands),
            }
            if let Some(queue) = Some(queue.raw()) {
                queue.submit(commands);
            } else {
                queue.write_buffer(target, 0, bytes);
            }
            if let Some(queue) = Some(queue.raw()) && { queue.submit(commands); true } {
                queue.write_buffer(target, 0, bytes);
            }
            queue.submit(commands);
        }
    "#,
    ));
    assert_eq!(
        sites.iter().map(|site| &site.receiver).collect::<Vec<_>>(),
        vec![
            &Receiver::Unknown,
            &Receiver::UploadQueue,
            &Receiver::Unknown,
            &Receiver::UploadQueue,
            &Receiver::Unknown,
            &Receiver::Unknown,
            &Receiver::UploadQueue,
        ]
    );
    assert_eq!(violations(&sites).len(), 4);
}
