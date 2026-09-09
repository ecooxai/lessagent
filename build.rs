fn main() {
    println!("cargo:rerun-if-changed=native/computer.swift");
    println!("cargo:rerun-if-changed=native/browser.swift");
    println!("cargo:rerun-if-changed=native/system.swift");
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
            .arg("-o")
            .arg(output)
            .status()
            .expect("macOS computer control requires Xcode command line tools (swiftc)");
        assert!(status.success(), "Failed to compile macOS computer helper");
    }
}
