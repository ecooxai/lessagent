use crate::{Result, err};
use base64::Engine;
use serde::Serialize;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub const LIGHT_LIMIT: u64 = 30_000;
const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_BUNDLE: usize = 64 * 1024 * 1024;
#[derive(Clone, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub kind: String,
    pub bytes: u64,
    pub tokens: u64,
}
#[derive(Clone, Default, Serialize)]
pub struct Inventory {
    pub files: Vec<FileInfo>,
    pub text_tokens: u64,
    pub image_tokens: u64,
    pub image_cost_usd: f64,
    pub total_tokens: u64,
    pub light_allowed: bool,
    pub warnings: Vec<String>,
    pub token_method: String,
}
#[derive(Clone)]
pub struct Image {
    pub path: PathBuf,
    pub mime: String,
    pub data: String,
}
pub struct Bundle {
    pub inventory: Inventory,
    pub text: String,
    pub images: Vec<Image>,
}
pub fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "svg" => "image/svg+xml",
        _ => "text/plain; charset=utf-8",
    }
}
fn internal(path: &Path) -> bool {
    let agent = Path::new("agent");
    let output = Path::new("agent/output");
    // The output tree is the one agent-owned tree that is part of model
    // context. Keep the `agent` directory itself traversable, while hiding
    // its other implementation/state directories and the archived snapshots.
    if path == agent {
        return false;
    }
    if path.starts_with(agent) {
        return !path.starts_with(output);
    }
    [".git"].iter().any(|p| path.starts_with(p))
}
pub fn default_ignores() -> Vec<String> {
    [
        "agent/context/",
        "agent/msgs/",
        "agent/continuity/",
        "agent/output-history/",
        ".gitignore",
        ".gitignore.*",
        "*.lock",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "LICENSE",
        "LICENSE.*",
        "license",
        "license.*",
        "LICENCE",
        "COPYING",
        "*.log",
        "*.tmp",
        "*.temp",
        "*.swp",
        "*.swo",
        "*~",
        ".DS_Store",
        "node_modules/",
        "target/",
        "dist/",
        "coverage/",
        ".cache/",
        "__pycache__/",
        ".venv/",
        "venv/",
        ".next/",
        ".nuxt/",
        ".pytest_cache/",
        "*.min.js",
        "*.min.css",
        "*.map",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
pub fn scan(root: &Path, include: bool) -> Result<Bundle> {
    scan_with_ignores(root, include, &default_ignores())
}
pub fn scan_with_ignores(root: &Path, include: bool, patterns: &[String]) -> Result<Bundle> {
    scan_for_model(root, include, patterns, "gpt-5.4")
}
pub fn scan_for_model(
    root: &Path,
    include: bool,
    patterns: &[String],
    model: &str,
) -> Result<Bundle> {
    let root = root.canonicalize()?;
    let mut inv = Inventory {
        token_method: format!(
            "o200k_base text tokenizer; 2048px / 2500-patch image estimate, multiplier 1 ($5 per 1M image tokens; selected model: {model}). File content only; actual request usage comes from the provider."
        ),
        ..Default::default()
    };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    let mut bundle_bytes: usize = 0;
    let mut ignores = ignore::gitignore::GitignoreBuilder::new(&root);
    for pattern in patterns {
        // Migrate the old blanket output exclusions to latest-output selection.
        if matches!(pattern.trim_matches('/'), "output" | "agent/output") {
            continue;
        }
        ignores.add_line(None, pattern)?;
    }
    let ignores = ignores.build()?;
    // The current `agent/output` tree is the hand-off between Light requests.
    // It is archived immediately after each response, so the next request can
    // safely include every file that remains here.  Keep the older top-level
    // `output` convention bounded to its newest child for compatibility with
    // existing workspaces; `agent/output-history` is excluded below.
    let project_output = root.join("output");
    let latest_project_output = std::fs::read_dir(&project_output)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .max_by_key(|entry| {
            (
                entry.metadata().and_then(|m| m.modified()).ok(),
                entry.file_name(),
            )
        })
        .map(|entry| entry.path());
    let current_agent_output = root.join("agent/output");
    let output_history = root.join("agent/output-history");
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        // Only project gitignore files and configured context exclusions apply.
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry({
            let root = root.clone();
            move |entry| {
                let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                let in_current_agent_output = entry.path().starts_with(&current_agent_output)
                    && !entry.path().starts_with(&output_history);
                let in_latest_project_output = latest_project_output
                    .as_ref()
                    .is_some_and(|latest| entry.path().starts_with(latest));
                (in_current_agent_output
                    || (!entry.path().starts_with(&project_output) || in_latest_project_output))
                    && !internal(relative)
                    && {
                        // All current Light output files are intentional context,
                        // including generated assets. Backend bookkeeping is
                        // stored under agent/continuity and never reaches this
                        // tree. Keep repository ignore files themselves out of
                        // the bundle, and never let this exception make
                        // output-history visible.
                        let is_gitignore_file = entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".gitignore");
                        in_current_agent_output && !is_gitignore_file
                            || !ignores
                                .matched(
                                    entry.path(),
                                    entry.file_type().is_some_and(|t| t.is_dir()),
                                )
                                .is_ignore()
                    }
            }
        })
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                inv.warnings.push(e.to_string());
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let p = entry.path();
        let rel = p.strip_prefix(&root)?.to_string_lossy().into_owned();
        let size = match p.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                inv.warnings.push(format!("{rel}: {e}"));
                continue;
            }
        };
        let media = mime(p);
        let is_image = matches!(
            media,
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        );
        let (kind, tokens) = if is_image {
            let tokens = match imagesize::size(p) {
                Ok(s) => match image_tokens(s.width as f64, s.height as f64, model) {
                    Some(n) => n,
                    None => {
                        inv.warnings.push(format!("{rel}: image token rules unavailable for {model}; conservative fallback"));
                        LIGHT_LIMIT
                    }
                },
                Err(e) => {
                    inv.warnings
                        .push(format!("{rel}: image dimensions unavailable: {e}"));
                    LIGHT_LIMIT
                }
            };
            inv.image_tokens = inv.image_tokens.saturating_add(tokens);
            if include
                && size <= MAX_FILE
                && bundle_bytes.saturating_add(size as usize) <= MAX_BUNDLE
            {
                bundle_bytes += size as usize;
                images.push(Image {
                    path: p.to_path_buf(),
                    mime: media.into(),
                    data: base64::engine::general_purpose::STANDARD.encode(std::fs::read(p)?),
                });
            } else if include {
                inv.warnings
                    .push(format!("{rel}: image exceeds bundle size limit"));
            }
            ("image", tokens)
        } else if media.starts_with("audio/") || media.starts_with("video/") {
            ("media", 0)
        } else {
            // Probe before reading large binary files. UTF-8 text files are counted in full.
            let mut f = std::fs::File::open(p)?;
            let mut probe = vec![0u8; 8192];
            let n = f.read(&mut probe)?;
            probe.truncate(n);
            if probe.contains(&0) {
                ("binary", 0)
            } else if size > MAX_FILE {
                inv.text_tokens = inv.text_tokens.saturating_add(size.div_ceil(4));
                inv.warnings.push(format!("{rel}: too large to bundle"));
                ("text", size.div_ceil(4))
            } else {
                let bytes = std::fs::read(p)?;
                match String::from_utf8(bytes) {
                    Ok(text) => {
                        let tokens = text_tokens(&text);
                        inv.text_tokens = inv.text_tokens.saturating_add(tokens);
                        if include {
                            if bundle_bytes + text.len() > MAX_BUNDLE {
                                inv.warnings.push("Project bundle exceeds 64 MiB".into());
                            } else {
                                bundle_bytes += text.len();
                                texts.push((rel.clone(), text));
                            }
                        }
                        ("text", tokens)
                    }
                    Err(_) => ("binary", 0),
                }
            }
        };
        if !matches!(kind, "text" | "image") {
            continue;
        }
        inv.files.push(FileInfo {
            path: rel,
            kind: kind.into(),
            bytes: size,
            tokens,
        });
    }
    inv.image_cost_usd = inv.image_tokens as f64 * 5. / 1_000_000.;
    inv.total_tokens = inv.text_tokens.saturating_add(inv.image_tokens);
    inv.light_allowed = inv.total_tokens < LIGHT_LIMIT && inv.warnings.is_empty();
    let mut text = String::from("PROJECT.txt\n\nFile structure (gitignore respected):\n");
    for f in &inv.files {
        text.push_str(&format!("{} [{}]\n", f.path, f.kind));
    }
    for (path, contents) in texts {
        text.push_str(&format!(
            "\n--- FILE: {path} ---\n{contents}\n--- END FILE ---\n"
        ));
    }
    Ok(Bundle {
        inventory: inv,
        text,
        images,
    })
}
pub fn resolve(root: &Path, relative: &str, write: bool) -> Result<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(err("Path must be relative and cannot traverse parents"));
    }
    let root = root.canonicalize()?;
    let path = root.join(rel);
    if path.exists() {
        if !path.canonicalize()?.starts_with(&root) {
            return Err(err("Path escapes workspace"));
        }
    } else if write {
        let mut ancestor = path.parent().ok_or_else(|| err("Invalid path"))?;
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| err("Invalid parent"))?;
        }
        if !ancestor.canonicalize()?.starts_with(&root) {
            return Err(err("Path escapes workspace"));
        }
    } else {
        return Err(err("File not found"));
    }
    Ok(path)
}
/// Exact file-content count for the o200k vocabulary; model envelopes are not included.
pub fn text_tokens(text: &str) -> u64 {
    static TOKENIZER: std::sync::OnceLock<tiktoken_rs::CoreBPE> = std::sync::OnceLock::new();
    TOKENIZER
        .get_or_init(|| tiktoken_rs::o200k_base().expect("bundled tokenizer"))
        .encode_ordinary(text)
        .len() as u64
}
/// Project image estimate: 2048px maximum side, 2500 32px patches, multiplier 1.
/// This is the configured budgeting rule, not provider-reported billing usage.
pub fn image_tokens(mut w: f64, mut h: f64, _model: &str) -> Option<u64> {
    if !w.is_finite() || !h.is_finite() || w <= 0. || h <= 0. {
        return None;
    }
    let scale = (2048. / w.max(h)).min(1.);
    w = (w * scale).floor().max(1.);
    h = (h * scale).floor().max(1.);
    let patches = |scale: f64| {
        ((w * scale).ceil().max(1.) / 32.).ceil() * ((h * scale).ceil().max(1.) / 32.).ceil()
    };
    if patches(1.) <= 2500. {
        return Some(patches(1.) as u64);
    }
    // Find the largest proportional resize whose rounded patch grid fits.
    let (mut low, mut high) = (0., 1.);
    for _ in 0..64 {
        let mid = (low + high) / 2.;
        if patches(mid) <= 2500. {
            low = mid;
        } else {
            high = mid;
        }
    }
    Some(patches(low) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignores_nested_files_and_blocks_escape() {
        let root = std::env::temp_dir().join(crate::id());
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join(".gitignore"), "*.secret\n").unwrap();
        std::fs::write(root.join("nested/.gitignore"), "skip\n").unwrap();
        std::fs::write(root.join("nested/skip"), "hidden").unwrap();
        std::fs::write(root.join("key.secret"), "secret").unwrap();
        std::fs::write(root.join("hello.rs"), "hello").unwrap();
        std::os::unix::fs::symlink("/tmp", root.join("outside")).unwrap();
        let b = scan(&root, true).unwrap();
        assert!(b.inventory.light_allowed);
        assert!(!b.text.contains("hidden"));
        assert!(!b.text.contains("key.secret"));
        assert!(b.text.contains("hello.rs"));
        assert!(resolve(&root, "../bad", false).is_err());
        assert!(resolve(&root, "outside/bad", true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn editable_defaults_exclude_noise() {
        let root = std::env::temp_dir().join(crate::id());
        std::fs::create_dir_all(root.join("nested")).unwrap();
        for name in ["Cargo.lock", "LICENSE", "nested/test.tmp", "source.rs"] {
            std::fs::write(root.join(name), "test").unwrap();
        }
        let default = scan(&root, true).unwrap();
        assert_eq!(default.inventory.files.len(), 1);
        assert_eq!(default.inventory.files[0].path, "source.rs");
        assert_eq!(
            scan_with_ignores(&root, true, &[])
                .unwrap()
                .inventory
                .files
                .len(),
            4
        );
        assert!(
            scan_with_ignores(&root, true, &["*.rs".into()])
                .unwrap()
                .inventory
                .files
                .iter()
                .all(|f| f.path != "source.rs")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn inventory_and_bundle_share_only_latest_output() {
        let root = std::env::temp_dir().join(crate::id());
        for dir in [
            "output/old",
            "agent/output/old",
            "output/latest",
            "agent/output/latest",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("result.txt"), dir).unwrap();
            let modified = if dir.ends_with("old") { 100 } else { 200 };
            std::fs::File::open(root.join(dir))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified),
                    ),
                )
                .unwrap();
        }
        std::fs::write(root.join(".gitignore"), "*.secret\n").unwrap();
        std::fs::write(root.join("output/latest/key.secret"), "secret").unwrap();
        std::fs::write(root.join("output/loose.txt"), "loose").unwrap();
        std::fs::write(root.join("output/latest/image.bin"), [0, 1, 2]).unwrap();
        let image = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jXioAAAAASUVORK5CYII=")
            .unwrap();
        std::fs::write(root.join("agent/output/latest/frame.png"), image).unwrap();
        let mut patterns = default_ignores();
        patterns.extend(["output/".into(), "agent/output/".into()]);
        let preview = scan_with_ignores(&root, false, &patterns).unwrap();
        let sent = scan_with_ignores(&root, true, &patterns).unwrap();
        let paths = |bundle: &Bundle| {
            bundle
                .inventory
                .files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&preview), paths(&sent));
        assert!(sent.text.contains("output/latest/result.txt"));
        assert!(sent.text.contains("agent/output/latest/result.txt"));
        // The current agent/output hand-off is included in full. Only the
        // archived output-history tree is omitted; the top-level output tree
        // retains its legacy newest-child behavior.
        assert!(
            sent.text
                .contains("--- FILE: agent/output/old/result.txt ---")
        );
        assert!(!sent.text.contains("--- FILE: output/old/result.txt ---"));
        assert!(!sent.text.contains("key.secret"));
        assert!(!sent.text.contains("loose.txt"));
        assert!(!sent.text.contains("image.bin"));
        assert!(
            sent.images
                .iter()
                .any(|image| image.path.ends_with("agent/output/latest/frame.png"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn current_agent_output_includes_all_iterations_and_files() {
        let root = std::env::temp_dir().join(crate::id());
        for session in ["chat-a", "chat-b"] {
            let base = root.join("agent/output").join(session);
            for (name, time) in [("step-0001", 100), ("step-0002", 200)] {
                let dir = base.join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("deliverable.txt"), format!("{session}/{name}")).unwrap();
                std::fs::File::open(&dir)
                    .unwrap()
                    .set_times(
                        std::fs::FileTimes::new().set_modified(
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(time),
                        ),
                    )
                    .unwrap();
            }
            std::fs::create_dir_all(root.join("agent/continuity").join(session)).unwrap();
            std::fs::write(
                root.join("agent/continuity")
                    .join(session)
                    .join("summary.md"),
                format!("Summary {session}"),
            )
            .unwrap();
            std::fs::write(
                root.join("agent/continuity")
                    .join(session)
                    .join("state.json"),
                format!("State {session}"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.join("agent/output-history/old-session")).unwrap();
        std::fs::write(
            root.join("agent/output-history/old-session/secret.txt"),
            "ARCHIVED_OUTPUT_MUST_NOT_BE_SENT",
        )
        .unwrap();
        let sent = scan(&root, true).unwrap();
        let preview = scan(&root, false).unwrap();
        // Every file in the current agent/output hand-off is available. Older
        // snapshots are removed by the Light loop before the next request and
        // live under agent/output-history when retained for inspection.
        assert_eq!(sent.inventory.files.len(), 4);
        assert_eq!(sent.inventory.total_tokens, preview.inventory.total_tokens);
        for session in ["chat-a", "chat-b"] {
            assert!(sent.text.contains(&format!("{session}/step-0001")));
            assert!(sent.text.contains(&format!("{session}/step-0002")));
        }
        assert!(!sent.text.contains("Summary chat-a"));
        assert!(!sent.text.contains("State chat-a"));
        assert!(!sent.text.contains("ARCHIVED_OUTPUT_MUST_NOT_BE_SENT"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn image_estimate_is_bounded_for_all_aspect_ratios() {
        for w in [1., 32., 33., 1024., 2048., 8192., 100000.] {
            for h in [1., 32., 33., 1024., 2048., 8192., 100000.] {
                let tokens = image_tokens(w, h, "gpt-4o-mini").unwrap();
                assert!((1..=2500).contains(&tokens), "{w}x{h}: {tokens}");
            }
        }
        assert_eq!(image_tokens(4096., 2048., "test"), Some(2048));
        assert_eq!(image_tokens(0., 32., "test"), None);
    }
    #[test]
    fn text_and_image_token_rules() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(93);
        assert!(text.len() > 4000);
        assert!(text_tokens(&text) > 800 && text_tokens(&text) < 1200);
        assert_eq!(image_tokens(1024., 1024., "gpt-5.4-mini"), Some(1024));
        assert_eq!(image_tokens(2048., 2048., "gpt-5.4"), Some(2500));
        assert_eq!(image_tokens(4096., 4096., "gpt-5.4"), Some(2500));
        assert_eq!(image_tokens(1024., 1024., "gpt-4o"), Some(1024));
        assert_eq!(image_tokens(1024., 1024., "unknown"), Some(1024));
    }
    #[test]
    fn threshold_is_strict() {
        let root = std::env::temp_dir().join(crate::id());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("x"), "x ".repeat(30_000)).unwrap();
        assert!(!scan(&root, false).unwrap().inventory.light_allowed);
        std::fs::remove_dir_all(root).unwrap();
    }
}
