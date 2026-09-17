fn fingerprint(bytes: &[u8], state: &mut u64) {
    // Reproducible change identifier, not a security hash.
    for byte in bytes {
        *state = (*state ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
}
fn source_files(dir: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("Cannot enumerate build sources") {
        let path = entry.expect("Cannot read source path").path();
        if path.is_dir() {
            source_files(&path, paths);
        } else {
            paths.push(path);
        }
    }
}
fn main() {
    let mut files = vec!["build.rs".into(), "Cargo.toml".into(), "Cargo.lock".into()];
    for dir in ["src", "native", "web", "docs"] {
        println!("cargo:rerun-if-changed={dir}");
        source_files(std::path::Path::new(dir), &mut files);
    }
    files.sort();
    let mut id = 0xcbf29ce484222325;
    for path in files {
        fingerprint(path.to_string_lossy().as_bytes(), &mut id);
        fingerprint(&[0], &mut id);
        fingerprint(
            &std::fs::read(path).expect("Cannot fingerprint source"),
            &mut id,
        );
    }
    let profile = std::env::var("PROFILE").unwrap();
    fingerprint(profile.as_bytes(), &mut id);
    println!("cargo:rustc-env=LESSAGENT_BUILD_ID={id:016x}");
    println!("cargo:rustc-env=LESSAGENT_BUILD_PROFILE={profile}");
    println!("cargo:rerun-if-changed=native/computer.swift");
    println!("cargo:rerun-if-changed=native/browser.swift");
    println!("cargo:rerun-if-changed=native/system.swift");
    println!("cargo:rerun-if-changed=native/window.swift");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
            .join("lessagent-computer");
        let main = output.with_file_name("main.swift");
        std::fs::copy("native/computer.swift", &main)
            .expect("Cannot prepare the native helper entrypoint");
        let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            other => panic!("Unsupported macOS architecture: {other}"),
        };
        let release = std::env::var("PROFILE").as_deref() == Ok("release");
        let status = std::process::Command::new("xcrun")
            .args([
                "swiftc",
                if release { "-O" } else { "-Onone" },
                "-g",
                "-target",
                &format!("{arch}-apple-macosx12.0"),
            ])
            .arg(main)
            .arg("native/browser.swift")
            .arg("native/system.swift")
            .arg("native/window.swift")
            .arg("native/keyboard_notice.swift")
            .arg("-o")
            .arg(&output)
            .status()
            .expect("macOS computer control requires Xcode command line tools (swiftc)");
        assert!(status.success(), "Failed to compile macOS computer helper");
        let mut helper_id = 0xcbf29ce484222325;
        fingerprint(
            &std::fs::read(output).expect("Cannot read native helper"),
            &mut helper_id,
        );
        println!("cargo:rustc-env=LESSAGENT_HELPER_BUILD_ID={helper_id:016x}");
    }
}
