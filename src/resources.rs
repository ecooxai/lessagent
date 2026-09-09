//! Server-owned resources only. Resource URIs are not arbitrary file or network
//! access, and no workspace, credentials, or computer-enable setting is needed.
use crate::state::App;
use serde_json::{Value, json};
use std::sync::Arc;

pub const INSTRUCTION_URI: &str = "lessagent://server/instruction.md";
pub const READ_FIRST: &str = "First read instruction.md at lessagent://server/instruction.md using resources/read, or the read_resource tool if the client only exposes tools. It contains live OS/CPU/GPU/RAM/display information, coding and computer-control guidance, 1000x600 browser defaults, project output/ deliverable rules, and the required final task summary. Discovery and resource reading are allowed before opening a workspace.";
pub type RpcError = (i32, &'static str);

fn no_cursor(params: &Value) -> Result<(), RpcError> {
    if !params.is_null() && !params.is_object() {
        return Err((-32602, "Params must be an object"));
    }
    if params.get("cursor").is_some() {
        return Err((
            -32602,
            "Invalid cursor: all resources are returned in a single page",
        ));
    }
    Ok(())
}
pub fn list(params: &Value) -> Result<Value, RpcError> {
    no_cursor(params)?;
    Ok(json!({"resources":[{
        "uri":INSTRUCTION_URI, "name":"instruction.md", "title":"Lessagent host information and operating instructions",
        "description":"Read first: live OS, CPU, GPU, RAM, screen resolution and window limits; coding, safe computer control, project output/ artifacts and final task summary.",
        "mimeType":"text/markdown", "annotations":{"audience":["assistant"],"priority":1.0}
    }]}))
}
pub fn templates(params: &Value) -> Result<Value, RpcError> {
    no_cursor(params)?;
    Ok(json!({"resourceTemplates":[]}))
}
fn validate_uri(params: &Value) -> Result<(), RpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((-32602, "Missing or invalid resource uri: expected a string"))?;
    if uri != INSTRUCTION_URI {
        return Err((
            -32002,
            "Resource not found; use resources/list to discover valid URIs",
        ));
    }
    Ok(())
}
pub async fn read(app: Arc<App>, params: &Value) -> Result<Value, RpcError> {
    validate_uri(params)?;
    let enabled = app.disk.lock().unwrap().settings.computer_enabled;
    // AppKit/Metal and filesystem probes never block the async request executor.
    let mut host = tokio::task::spawn_blocking(snapshot)
        .await
        .map_err(|_| (-32603, "Host information probe failed"))?;
    host["computer_control_enabled"] = json!(enabled);
    Ok(contents(host))
}
fn contents(host: Value) -> Value {
    let text = format!(
        "{}\n## Live host system information\n\nThis snapshot is collected when the resource is read. Empty arrays or explicit Unavailable notes mean the host could not report that information; do not infer missing hardware or resolution. No credentials, user/host names, serial numbers, window titles, or screenshots are collected.\n\n```json\n{}\n```\n",
        include_str!("../docs/instruction.md"),
        serde_json::to_string_pretty(&host).unwrap()
    );
    json!({"contents":[{"uri":INSTRUCTION_URI, "mimeType":"text/markdown", "text":text,
        "_meta":{"lessagent/system":host}}]})
}
fn snapshot() -> Value {
    #[cfg(target_os = "macos")]
    let mut host = crate::computer::host_info().unwrap_or_else(|_| json!({
        "probe_status":"Unavailable: native host probe could not complete", "cpu":{"model":"Unavailable"},
        "ram":{"status":"Unavailable"}, "gpus":[], "displays":[]}));
    #[cfg(not(target_os = "macos"))]
    let mut host = linux_snapshot();
    host["os"] = json!(std::env::consts::OS);
    host["process_architecture"] = json!(std::env::consts::ARCH);
    host["observed_at_unix_ms"] = json!(crate::now());
    host["browser_size_schema"] = crate::computer::browser_size_schema();
    if let Some(bytes) = host["ram"]["total_bytes"].as_u64() {
        host["ram"]["total_gib"] = json!(bytes as f64 / 1_073_741_824.0);
    }
    host
}

#[cfg(any(not(target_os = "macos"), test))]
fn proc_fields(cpu: &str, memory: &str) -> Value {
    let model = cpu
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| matches!(key.trim(), "model name" | "Hardware" | "Processor"))
        .map(|(_, value)| value.trim())
        .unwrap_or("Unavailable");
    let mut ram = json!({});
    for (name, field) in [
        ("MemTotal", "total_bytes"),
        ("MemAvailable", "available_bytes"),
    ] {
        let bytes = memory
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| {
                let mut parts = value.split_whitespace();
                let kb = parts.next()?.parse::<u64>().ok()?;
                (parts.next()? == "kB")
                    .then(|| kb.checked_mul(1024))
                    .flatten()
            });
        if let Some(bytes) = bytes {
            ram[field] = json!(bytes);
        }
    }
    if ram.get("total_bytes").is_none() {
        ram["status"] = json!("Unavailable");
    }
    json!({"cpu":{"model":model}, "ram":ram})
}
#[cfg(not(target_os = "macos"))]
fn linux_snapshot() -> Value {
    let mut host = proc_fields(
        &std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default(),
        &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
    );
    if let Ok(cores) = std::thread::available_parallelism() {
        host["cpu"]["available_logical_cores"] = json!(cores.get());
    }
    host["os_version"] = json!(
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_else(|_| "Unavailable".into())
            .trim()
    );
    let mut gpus = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(index) = name.to_str().and_then(|name| name.strip_prefix("card")) else {
                continue;
            };
            if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let device = entry.path().join("device");
            let vendor = std::fs::read_to_string(device.join("vendor"));
            let id = std::fs::read_to_string(device.join("device"));
            if let (Ok(vendor), Ok(id)) = (vendor, id) {
                gpus.push(json!({"pci_vendor_id":vendor.trim(), "pci_device_id":id.trim()}));
            }
        }
    }
    host["gpus"] = json!(gpus);
    host["gpu_note"] =
        json!("GPU PCI identifiers from DRM; model names and VRAM may be unavailable.");
    host["displays"] = json!([]);
    host["display_note"] = json!(
        "Unavailable: this backend has no Linux display-geometry probe. Use an explicit screenshot's encoded width/height. Managed browser_open is macOS-only."
    );
    host
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_single_page_and_bootstrap_readable() {
        let listed = list(&json!({})).unwrap();
        assert_eq!(listed["resources"][0]["name"], "instruction.md");
        assert_eq!(listed["resources"][0]["uri"], INSTRUCTION_URI);
        assert_eq!(listed["resources"][0]["mimeType"], "text/markdown");
        assert!(listed.get("nextCursor").is_none());
        assert_eq!(
            templates(&Value::Null).unwrap(),
            json!({"resourceTemplates":[]})
        );
        for cursor in [
            json!("unknown"),
            json!(""),
            json!(false),
            json!(7),
            Value::Null,
        ] {
            assert_eq!(list(&json!({"cursor":cursor})).unwrap_err().0, -32602);
            assert_eq!(templates(&json!({"cursor":cursor})).unwrap_err().0, -32602);
        }
    }
    #[test]
    fn uri_allowlist_never_reads_paths_or_fetches_network_content() {
        assert!(validate_uri(&json!({"uri":INSTRUCTION_URI})).is_ok());
        for uri in [
            "file:///etc/passwd",
            "https://example.com/",
            "../instruction.md",
            "instruction.md",
            "lessagent://server/instruction.md?secret=x",
            "lessagent://server/../instruction.md",
            "",
        ] {
            assert_eq!(validate_uri(&json!({"uri":uri})).unwrap_err().0, -32002);
        }
        for params in [
            Value::Null,
            json!({}),
            json!({"uri":false}),
            json!({"uri":7}),
        ] {
            assert_eq!(validate_uri(&params).unwrap_err().0, -32602);
        }
    }
    #[test]
    fn resource_contains_workflows_and_honest_unavailable_snapshot() {
        let result = contents(json!({"os":"test", "cpu":{"model":"Unavailable"},"displays":[]}));
        let item = &result["contents"][0];
        let text = item["text"].as_str().unwrap();
        for marker in [
            "Agents.md",
            "1000 by 600",
            "project directory's `output/`",
            "3D",
            "final summary",
            "cargo build --locked",
            "window_id",
            "screen_width",
            "Unavailable",
            "Live host system information",
        ] {
            assert!(text.contains(marker), "{marker}");
        }
        assert_eq!(item["_meta"]["lessagent/system"]["os"], "test");
    }
    #[test]
    fn linux_memory_units_and_missing_values_are_not_fabricated() {
        let parsed = proc_fields(
            "model name : Fixture CPU\n",
            "MemTotal: 8192 kB\nMemAvailable: 2048 kB\n",
        );
        assert_eq!(parsed["cpu"]["model"], "Fixture CPU");
        assert_eq!(parsed["ram"]["total_bytes"], 8388608);
        assert_eq!(parsed["ram"]["available_bytes"], 2097152);
        assert!(proc_fields("", "MemTotal: 42 MB")["ram"]["total_bytes"].is_null());
        assert!(
            proc_fields("", "MemTotal: 18446744073709551615 kB")["ram"]["total_bytes"].is_null()
        );
    }
}
