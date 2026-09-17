//! Recipient-verified GUI regression; runs after the unit suite on local macOS.
#[cfg(target_os = "macos")]
#[test]
fn blender_modeling_and_chrome_drawing() {
    if std::env::var_os("CI").is_some() || std::env::var_os("LESSAGENT_SKIP_GUI_TESTS").is_some() {
        eprintln!("SKIP modeling GUI regression: CI or LESSAGENT_SKIP_GUI_TESTS");
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("python3")
        .arg(root.join("tests/background_modeling.py"))
        .arg(env!("CARGO_BIN_EXE_lessagent"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(root)
        .output()
        .expect("python3 is required for the modeling GUI regression");
    assert!(
        output.status.success(),
        "Modeling regression failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"passed\": true"));
}
