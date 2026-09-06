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
    [
        ".git",
        "agent/context",
        "agent/knowledge/done",
        "agent/captures",
    ]
    .iter()
    .any(|p| path.starts_with(p))
}
/// One UTF-8 byte per text token is deliberately conservative, independent of provider tokenizers.
/// Vision budget takes the larger of patch and tile estimates; billing varies by model.
pub fn default_ignores() -> Vec<String> {
    [
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
    let root = root.canonicalize()?;
    let mut inv=Inventory { token_method:"Conservative estimate: UTF-8 bytes for text; max(patches, tiles) for images. Not billing tokens.".into(), ..Default::default() };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    let mut bundle_bytes: usize = 0;
    let mut ignores = ignore::gitignore::GitignoreBuilder::new(&root);
    for pattern in patterns {
        ignores.add_line(None, pattern)?;
    }
    let ignores = ignores.build()?;
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false)
        .follow_links(false)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry({
            let root = root.clone();
            move |entry| {
                !internal(entry.path().strip_prefix(&root).unwrap_or(entry.path()))
                    && !ignores
                        .matched(entry.path(), entry.file_type().is_some_and(|t| t.is_dir()))
                        .is_ignore()
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
                Ok(s) => {
                    let w = s.width as u64;
                    let h = s.height as u64;
                    (w.div_ceil(32) * h.div_ceil(32) * 3)
                        .max(85 + 170 * w.div_ceil(512) * h.div_ceil(512))
                }
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
                inv.text_tokens = inv.text_tokens.saturating_add(size);
                inv.warnings.push(format!("{rel}: too large to bundle"));
                ("text", size)
            } else {
                let bytes = std::fs::read(p)?;
                match String::from_utf8(bytes) {
                    Ok(text) => {
                        inv.text_tokens = inv.text_tokens.saturating_add(size);
                        if include {
                            if bundle_bytes + text.len() > MAX_BUNDLE {
                                inv.warnings.push("Project bundle exceeds 64 MiB".into());
                            } else {
                                bundle_bytes += text.len();
                                texts.push((rel.clone(), text));
                            }
                        }
                        ("text", size)
                    }
                    Err(_) => ("binary", 0),
                }
            }
        };
        inv.files.push(FileInfo {
            path: rel,
            kind: kind.into(),
            bytes: size,
            tokens,
        });
    }
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
    fn threshold_is_strict() {
        let root = std::env::temp_dir().join(crate::id());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("x"), vec![b'x'; 30_000]).unwrap();
        assert!(!scan(&root, false).unwrap().inventory.light_allowed);
        std::fs::remove_dir_all(root).unwrap();
    }
}
