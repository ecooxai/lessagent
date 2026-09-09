#![recursion_limit = "256"]

pub mod agent;
pub mod computer;
pub mod context;
pub mod git;
pub mod image_content;
pub mod mcp;
pub mod provider;
pub mod resources;
pub mod server;
pub mod state;
pub mod terminal;
pub mod tools;
pub mod tui;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
pub fn err(message: impl Into<String>) -> Error {
    std::io::Error::other(message.into()).into()
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn id() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .expect("OS random source unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.into();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &text[..end])
}

/// Keep the newest output, including the result of a long-running command.
pub fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.into();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("[earlier output truncated]\n{}", &text[start..])
}
