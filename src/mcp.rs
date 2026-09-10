use crate::state::App;
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};

const MAX_SUMMARY_CHARS: usize = 1000;
const WORKSPACE_INSTRUCTIONS: &str = "Before inspecting project files, running commands, editing, or delegating work, read the workspace-root Agents.md with read_file first. If it is absent, try AGENTS.md; if neither exists, report that and continue. Follow has_more/next_offset until the entire guidance file has been read. Read applicable nested Agents.md/AGENTS.md before working in a subdirectory. Opening or listing workspaces and reading guidance are bootstrap steps allowed before other project work.";
const SUMMARY_INSTRUCTIONS: &str = "Every tool call must include summary: a concise, nonblank paragraph (at most 1000 characters). State what this specific call will do and why, then include current task progress in the same paragraph: a progress score such as Progress 60/100, what has already been completed, and what this call advances next. Describe intended action and verified prior progress only; never claim this call succeeded before observing its result, and do not include secrets. The summary is client-provided metadata, not executable input.";

fn server_instructions() -> String {
    format!(
        "Local computer and coding agent. {} Open a folder with workspace_open or select one with workspace_list, then pass its workspace id to project tools. {WORKSPACE_INSTRUCTIONS} {SUMMARY_INSTRUCTIONS} Commands execute on the host.",
        crate::resources::READ_FIRST
    )
}

/// Apply the MCP-only contract centrally, including tools without a workspace.
/// Internal agent/CLI tool definitions deliberately keep their existing schema.
fn tool_definitions() -> Vec<Value> {
    let mut defs = crate::tools::definitions();
    for d in &mut defs {
        add_required_parameter(
            &mut d["parameters"],
            "workspace",
            json!({
                "type":"string", "description":"Workspace id returned by workspace_open/list"
            }),
        );
    }
    defs.extend([
        json!({"name":"workspace_open","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
        json!({"name":"workspace_list","parameters":{"type":"object","properties":{}}}),
        json!({"name":"list_resources","parameters":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"read_resource","parameters":{"type":"object","properties":{"uri":{"type":"string","description":"Exact URI from list_resources; use lessagent://server/instruction.md for host information and server guidance."}},"required":["uri"],"additionalProperties":false}})
    ]);
    defs.into_iter().map(|mut d| {
        add_required_parameter(&mut d["parameters"], "summary", json!({
            "type":"string", "minLength":1, "maxLength":MAX_SUMMARY_CHARS, "pattern":r"\S",
            "description":SUMMARY_INSTRUCTIONS
        }));
        let name = d["name"].as_str().unwrap_or_default();
        json!({
            "name":name,
            "description":mcp_tool_description(name, d["description"].as_str().unwrap_or_default()),
            "inputSchema":d["parameters"],
            "annotations":tool_annotations(name)
        })
    }).collect()
}

fn tool_annotations(name: &str) -> Value {
    let read_only = matches!(
        name,
        "read_file"
            | "list_files"
            | "workspace_list"
            | "terminal_read"
            | "list_resources"
            | "read_resource"
            | "get_screenshot"
            | "list_windows"
    );
    let non_destructive = read_only || matches!(name, "workspace_open" | "browser_open");
    let closed_world = read_only || matches!(name, "write_file" | "write_image" | "terminal_stop");
    json!({"readOnlyHint":read_only, "destructiveHint":!non_destructive,
        "idempotentHint":read_only || matches!(name, "workspace_open" | "write_file" | "write_image" | "terminal_stop"),
        "openWorldHint":!closed_world})
}

fn add_required_parameter(schema: &mut Value, name: &str, property: Value) {
    schema["properties"][name] = property;
    if !schema["required"].is_array() {
        schema["required"] = json!([]);
    }
    let required = schema["required"].as_array_mut().unwrap();
    if !required.iter().any(|value| value == name) {
        required.push(json!(name));
    }
}

fn call_summary(arguments: &Value) -> crate::Result<&str> {
    if !arguments.is_object() {
        return Err(crate::err("Tool arguments must be an object"));
    }
    let summary = arguments["summary"].as_str().ok_or_else(|| crate::err(
        "Missing or invalid summary: include a nonblank string explaining what this tool call will do and why"
    ))?;
    if summary.trim().is_empty() {
        return Err(crate::err(
            "summary must contain non-whitespace text explaining this tool call",
        ));
    }
    // JSON Schema maxLength counts Unicode characters, not UTF-8 bytes.
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(crate::err(format!(
            "summary must be at most {MAX_SUMMARY_CHARS} characters"
        )));
    }
    Ok(summary.trim())
}
pub async fn handle(app: Arc<App>, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    if request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || id
            .as_ref()
            .is_some_and(|id| !id.is_string() && !id.is_number())
    {
        return Some(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid Request"}}),
        );
    }
    let method = request["method"].as_str().unwrap();
    id.as_ref()?;
    let params = &request["params"];
    if !params.is_null() && !params.is_object() {
        return Some(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Params must be an object"}}),
        );
    }
    let result = match method {
        "initialize" => Ok(json!({"protocolVersion":params["protocolVersion"].as_str()
                .filter(|v| matches!(*v, "2025-03-26" | "2025-06-18" | "2025-11-25"))
                .unwrap_or("2025-11-25"),"capabilities":{"tools":{"listChanged":false},"resources":{"subscribe":false,"listChanged":false}},"serverInfo":{"name":"lessagent","version":env!("CARGO_PKG_VERSION")},"instructions":server_instructions()})),
        "ping" => Ok(json!({})),
        "resources/list" => crate::resources::list(params),
        "resources/templates/list" => crate::resources::templates(params),
        "resources/read" => crate::resources::read(app.clone(), params).await,
        "tools/list" => Ok(json!({"tools":tool_definitions()})),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let started = Instant::now();
            let result = call(app, params).await;
            let time_cost_ms = started.elapsed().as_millis() as u64;
            let (mut v, is_error) = match result {
                Ok(v) => (v, false),
                Err(e) => (json!({"error":e.to_string()}), true),
            };
            enrich_file_result(name, &mut v);
            let image = v.as_object_mut().and_then(|o| o.remove("image"));
            let content = mcp_result_content(&v, image, is_error, time_cost_ms);
            let mut structured = json!({"result":v,"time_cost_ms":time_cost_ms});
            if !is_error && matches!(name, "workspace_open" | "workspace_list") {
                structured["instructions"] = json!(format!(
                    "{} {WORKSPACE_INSTRUCTIONS}",
                    crate::resources::READ_FIRST
                ));
            }
            Ok(json!({"content":content,"structuredContent":structured,"isError":is_error}))
        }
        _ => Err((-32601, "Method not found")),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
        Err((code, message)) => {
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
        }
    })
}
fn mcp_tool_description(name: &str, fallback: &str) -> String {
    let (summary, purpose, how) = match name {
        "list_resources" => (
            "List server-owned MCP resources, including instruction.md.",
            "Use it for resource discovery when the client only exposes MCP tools; equivalent to resources/list.",
            "Pass summary; no workspace is required. Read instruction.md with read_resource before coding or computer work. This tool is read-only.",
        ),
        "read_resource" => (
            "Read instruction.md with live host system information and server usage guidance.",
            "Use it to learn OS, CPU, GPU, RAM, screen geometry, output rules and safe coding/computer workflows; equivalent to resources/read.",
            "Pass summary and uri from list_resources (lessagent://server/instruction.md). No workspace or computer-control permission is required. Returns Markdown and structured system metadata; refresh after display changes.",
        ),
        "get_screenshot" => (
            "Capture a standalone read-only screenshot.",
            "Use it for the initial window view or recovery from screenshot_error, with the same standalone screenshot behavior and metadata returned after virtual input.",
            "Pass workspace and optional capture_path/show_pointer. For a macOS window pass exact window_id and pid from list_windows. Image pixel dimensions are authoritative. Stage Manager can require a perspective-corrected low-resolution thumbnail; inspect capture_quality. Capture never activates the app.",
        ),
        "virtual_pointer" => (
            "Control a standalone background virtual pointer.",
            "Use move, click, drag or scroll on the selected window without moving the physical pointer.",
            "Pass action, window_id/pid on macOS, image pixel coordinates and screen_width/screen_height. Drag accepts a continuous path. Every input returns a fresh screenshot. Chrome requires browser_open; native windows use exact-ID geometry. Never replay input solely because capture failed.",
        ),
        "virtual_keyboard" => (
            "Type text or press a key chord in a background window.",
            "Use it after selecting the intended input with virtual_pointer. Keyboard input is separate from pointer and screenshot tools.",
            "Pass action:type with text, or action:key with key (such as cmd+a), plus window_id/pid on macOS. Never supply both text and key. Returns a fresh automatic screenshot; no global-input fallback.",
        ),
        "list_windows" => (
            "List existing windows without activation.",
            "Use it to select the exact existing window and owner for background control.",
            "Pass workspace, then reuse window_id and pid with get_screenshot, virtual_pointer and virtual_keyboard. Window listing is read-only; screenshot the target before choosing coordinates.",
        ),
        "app_open" => (
            "Open or reuse a native application without requesting foreground focus.",
            "Use it for native apps such as Blender; Chrome must use browser_open instead.",
            "Pass app (name, bundle ID or .app path). new_instance defaults true; false reuses one unambiguous existing window. Blender launches with --no-window-focus. Returns IDs and an automatic screenshot. Other apps may ignore nonactivation; inspect focus_changed. Never restarts an existing app.",
        ),
        "browser_open" => (
            "Open a new background Chrome window in the existing persistent managed profile by default, with a default size of 1000 by 600 logical points.",
            "Use it to create another controlled window while preserving cookies/storage from Lessagent's existing managed Chrome profile and leaving the user's normal browser and physical pointer untouched.",
            "Pass workspace, summary and an HTTP(S) url; include a purpose query parameter describing why this window exists and the model name, using ?purpose=texttodescribepurposeofthiswindow_by_modelname (or &purpose=... when the URL already has a query string, with the value URL-encoded). Optional width/height must not exceed the current primary display's logical resolution. browser_open creates a new window but reuses the persistent Lessagent-managed user-data directory by default, and reuses its live Chrome process when available; it creates a new managed profile only when none exists. Reuse returned window_id/pid and image width/height with virtual_pointer/virtual_keyboard. Screenshots default to project output/computer/.",
        ),
        "shell" => (
            "Run Bash in a visible persistent workspace terminal.",
            "Use it for interactive or long-running shell work that should remain attached to a PTY.",
            "Pass workspace and command. If it keeps running, use terminal_read for new output, terminal_write for input, and terminal_stop to end it.",
        ),
        "bash" => (
            "Run Bash code in a workspace PTY.",
            "Use it for builds, tests, searches, scripts, and other host shell commands.",
            "Pass workspace and command, optionally wait_ms. Read output and exit status from the response; use terminal_read/write/stop if the process is still running.",
        ),
        "python" => (
            "Run Python 3 code in a new unbuffered workspace PTY.",
            "Use it for local scripting, data processing, checks, and small automation tasks.",
            "Pass workspace and code, optionally wait_ms. Read output and exit status from the response; use terminal_read/write/stop if it remains active.",
        ),
        "write_image" => (
            "Save an MCP image content block into the workspace and return the image.",
            "Use it to persist generated or transferred PNG, JPEG, GIF, or WebP images as workspace artifacts.",
            "Pass workspace, a workspace-relative path, and an image block whose mime type matches the file extension.",
        ),
        "terminal_read" => (
            "Read recent output and status from a workspace terminal.",
            "Use it to follow a command that did not finish during its initial shell, bash, or python call.",
            "Pass workspace and terminal_id, optionally wait_ms. Stop polling once the process exits or when a running service can be checked directly.",
        ),
        "terminal_write" => (
            "Send text or control characters to a workspace terminal.",
            "Use it to answer prompts or interact with a running PTY process.",
            "Pass workspace, terminal_id, and text. Include a newline when the program expects Enter.",
        ),
        "terminal_stop" => (
            "Stop a running workspace terminal process.",
            "Use it to terminate a server, search, script, or other PTY command that should no longer run.",
            "Pass workspace and terminal_id, then confirm the returned terminal status.",
        ),
        "read_file" => (
            "Read a workspace text file excerpt or return an image as an MCP image block.",
            "Use it to inspect only the file range needed for the current task without flooding the tool response.",
            "Pass workspace and path. Text defaults to a 4 KB excerpt and is capped at 8 KB per MCP call; use offset and limit to paginate. Images are returned as image content blocks.",
        ),
        "write_file" => (
            "Create or replace a UTF-8 text file in the workspace.",
            "Use it to save deliberate text edits or new source/configuration files.",
            "Read an existing file before replacing it, then pass workspace, path, and the complete replacement text.",
        ),
        "list_files" => (
            "List workspace files with context token estimates while respecting ignore rules.",
            "Use it to discover the project layout and choose which files need inspection.",
            "Pass workspace. Use read_file with bounded ranges for the specific files you need next.",
        ),
        "workspace_open" => (
            "Open an existing absolute local folder as a Lessagent workspace.",
            "Use it to establish the workspace id required by project-scoped MCP tools.",
            "Pass an absolute path once, then reuse the returned workspace id in subsequent calls.",
        ),
        "workspace_list" => (
            "List the workspaces currently known to Lessagent.",
            "Use it to recover an existing workspace id instead of opening the same folder again.",
            "Pass summary, then select the workspace whose path matches the task and read its Agents.md first.",
        ),
        _ => (
            if fallback.is_empty() {
                "Run a Lessagent MCP operation."
            } else {
                fallback
            },
            "Use this tool for the workspace operation described in its summary.",
            "Call it with the fields in the input schema and inspect the returned status or output before continuing.",
        ),
    };
    format!(
        "Summary: {summary}\n\nPurpose: {purpose}\n\nHow: {how}\n\nCall summary: {SUMMARY_INSTRUCTIONS}\n\nRead first: {} {WORKSPACE_INSTRUCTIONS}",
        crate::resources::READ_FIRST
    )
}

fn enrich_file_result(name: &str, value: &mut Value) {
    if name != "read_file" || !value["text"].is_string() {
        return;
    }
    let text = value["text"].as_str().unwrap_or_default();
    let source = text.strip_suffix("\n[truncated]").unwrap_or(text);
    let offset = value["offset"].as_u64().unwrap_or(0);
    let total = value["total_bytes"].as_u64().unwrap_or(0);
    let returned = source.len() as u64;
    value["returned_bytes"] = json!(returned);
    value["next_offset"] = json!((offset + returned).min(total));
    value["has_more"] = json!(offset + returned < total);
}

fn mcp_result_content(
    value: &Value,
    image: Option<Value>,
    is_error: bool,
    time_cost_ms: u64,
) -> Vec<Value> {
    let mut content = Vec::new();
    let text = if is_error {
        format!(
            "Result: error · {time_cost_ms} ms\n{}",
            value["error"].as_str().unwrap_or("tool call failed")
        )
    } else {
        format!("Result: ok · {time_cost_ms} ms")
    };
    content.push(json!({"type":"text","text":text}));
    if let Some(i) = image {
        content.push(crate::image_content::mcp_block(&i));
    }
    content
}

async fn call(app: Arc<App>, p: &Value) -> crate::Result<Value> {
    let name = crate::tools::string(p, "name")?;
    let mut arguments = p.get("arguments").cloned().unwrap_or_else(|| json!({}));
    // Validate before any workspace/file/process/agent side effects.
    call_summary(&arguments)?;
    // Removed public MCP-only surfaces are rejected before workspace dispatch.
    // Their internal backend capabilities remain available to the normal Lessagent runtime.
    if name == "computer" {
        return Err(crate::err(
            "Unknown tool: computer; use the standalone GUI tools instead",
        ));
    }
    if matches!(name, "agent_run" | "agent_status") {
        return Err(crate::err(format!("Unknown tool: {name}")));
    }
    // Metadata must never reach shell code, delegated prompts, or GUI input.
    arguments.as_object_mut().unwrap().remove("summary");
    if name == "read_file" {
        const DEFAULT_MCP_FILE_BYTES: u64 = 4_000;
        const MAX_MCP_FILE_BYTES: u64 = 8_000;
        let limit = arguments["limit"]
            .as_u64()
            .unwrap_or(DEFAULT_MCP_FILE_BYTES)
            .clamp(1, MAX_MCP_FILE_BYTES);
        arguments["limit"] = json!(limit);
    }
    let a = &arguments;
    if matches!(
        name,
        "list_resources" | "read_resource" | "workspace_open" | "workspace_list"
    ) {
        app.log_with_details(
            "tool",
            &format!("{name} · MCP"),
            Some(crate::tools::tool_log_details(name, a)),
        );
    }
    match name {
        "list_resources" => crate::resources::list(a).map_err(|(_, message)| crate::err(message)),
        "read_resource" => crate::resources::read(app, a)
            .await
            .map_err(|(_, message)| crate::err(message)),
        "workspace_open" => Ok(serde_json::to_value(
            app.open_workspace(std::path::Path::new(crate::tools::string(a, "path")?))?,
        )?),
        "workspace_list" => Ok(json!(app.disk.lock().unwrap().workspaces)),
        _ => crate::tools::execute(app, crate::tools::string(a, "workspace")?, name, a).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestApp {
        app: Arc<App>,
        root: std::path::PathBuf,
    }
    impl TestApp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("lessagent-mcp-{}", crate::id()));
            let app = App::load(root.join("data")).unwrap();
            Self { app, root }
        }
        async fn request(&self, method: &str, params: Value) -> Value {
            handle(
                self.app.clone(),
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            )
            .await
            .unwrap()
        }
        async fn tool(&self, name: &str, arguments: Value) -> Value {
            self.request("tools/call", json!({"name":name,"arguments":arguments}))
                .await["result"]
                .clone()
        }
    }
    impl Drop for TestApp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn every_mcp_tool_requires_summary_without_changing_internal_tools() {
        let definitions = tool_definitions();
        let internal = crate::tools::definitions();
        assert_eq!(definitions.len(), internal.len() + 4);
        let mut names = std::collections::HashSet::new();
        for tool in &definitions {
            assert!(names.insert(tool["name"].as_str().unwrap()));
            let schema = &tool["inputSchema"];
            let summary = &schema["properties"]["summary"];
            assert_eq!(summary["type"], "string");
            assert_eq!(summary["minLength"], 1);
            assert_eq!(summary["maxLength"], MAX_SUMMARY_CHARS);
            assert_eq!(summary["pattern"], r"\S");
            let required = schema["required"].as_array().unwrap();
            assert_eq!(required.iter().filter(|v| **v == "summary").count(), 1);
            let description = tool["description"].as_str().unwrap();
            for marker in [
                "Summary: ",
                "\n\nPurpose: ",
                "\n\nHow: ",
                SUMMARY_INSTRUCTIONS,
                WORKSPACE_INSTRUCTIONS,
            ] {
                assert!(description.contains(marker), "{}: {marker}", tool["name"]);
            }
        }
        for tool in internal {
            assert!(tool["parameters"]["properties"].get("summary").is_none());
            let mcp = definitions
                .iter()
                .find(|d| d["name"] == tool["name"])
                .unwrap();
            for (key, value) in tool["parameters"]["properties"].as_object().unwrap() {
                assert_eq!(&mcp["inputSchema"]["properties"][key], value);
            }
            if let Some(required) = tool["parameters"]["required"].as_array() {
                for key in required {
                    assert!(
                        mcp["inputSchema"]["required"]
                            .as_array()
                            .unwrap()
                            .contains(key)
                    );
                }
            }
            assert!(
                mcp["inputSchema"]["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("workspace"))
            );
        }
    }

    #[tokio::test]
    async fn aggregate_computer_tool_is_not_advertised_or_callable_over_mcp() {
        assert!(
            tool_definitions()
                .iter()
                .all(|tool| tool["name"] != "computer")
        );
        let fixture = TestApp::new();
        let result = fixture
            .tool(
                "computer",
                json!({"summary":"Progress 1/100 — done: MCP initialized. Next: verify the removed aggregate name is rejected."}),
            )
            .await;
        assert_eq!(result["isError"], true);
        assert!(
            result["structuredContent"]["result"]["error"]
                .as_str()
                .unwrap()
                .contains("Unknown tool: computer")
        );
    }

    #[tokio::test]
    async fn agent_job_tools_are_not_advertised_or_callable_over_mcp() {
        for name in ["agent_run", "agent_status"] {
            assert!(tool_definitions().iter().all(|tool| tool["name"] != name));
        }
        let fixture = TestApp::new();
        for (name, arguments) in [
            (
                "agent_run",
                json!({"workspace":"unused","prompt":"unused","summary":"Progress 1/100 — done: MCP initialized. Next: verify removed agent_run is rejected."}),
            ),
            (
                "agent_status",
                json!({"job_id":"unused","summary":"Progress 1/100 — done: MCP initialized. Next: verify removed agent_status is rejected."}),
            ),
        ] {
            let result = fixture.tool(name, arguments).await;
            assert_eq!(result["isError"], true, "{name}: {result}");
            assert_eq!(
                result["structuredContent"]["result"]["error"],
                format!("Unknown tool: {name}")
            );
        }
        assert!(fixture.app.disk.lock().unwrap().jobs.is_empty());
    }

    #[test]
    fn summary_validation_matches_schema_limits_and_handles_unicode() {
        for invalid in [
            Value::Null,
            json!([]),
            json!({}),
            json!({"summary":null}),
            json!({"summary":7}),
            json!({"summary":false}),
            json!({"summary":[]}),
            json!({"summary":{}}),
            json!({"summary":""}),
            json!({"summary":" \t\n\r\u{2003}"}),
            json!({"summary":"a".repeat(MAX_SUMMARY_CHARS + 1)}),
            json!({"summary":"雪".repeat(MAX_SUMMARY_CHARS + 1)}),
        ] {
            assert!(call_summary(&invalid).is_err(), "accepted {invalid}");
        }
        for valid in [
            "x".to_owned(),
            "Read the guidance.\nFollow its test commands.".to_owned(),
            "雪".repeat(MAX_SUMMARY_CHARS),
            "🦀".repeat(MAX_SUMMARY_CHARS),
        ] {
            assert_eq!(call_summary(&json!({"summary":valid})).unwrap(), valid);
        }
        assert_eq!(
            call_summary(&json!({"summary":"  Read Agents.md first.  "})).unwrap(),
            "Read Agents.md first."
        );
        // Length is measured before trimming, just like the advertised schema.
        assert!(
            call_summary(&json!({"summary":format!("x{}", " ".repeat(MAX_SUMMARY_CHARS))}))
                .is_err()
        );
    }

    #[test]
    fn adding_required_parameters_is_idempotent() {
        let mut schema = json!({"type":"object","properties":{}});
        for _ in 0..2 {
            add_required_parameter(&mut schema, "summary", json!({"type":"string"}));
        }
        assert_eq!(schema["required"], json!(["summary"]));
    }

    #[tokio::test]
    async fn handshake_and_workspace_bootstrap_include_read_first_guidance() {
        let fixture = TestApp::new();
        for version in ["2025-03-26", "2025-06-18", "2025-11-25", "unknown"] {
            let response = fixture
                .request("initialize", json!({"protocolVersion":version}))
                .await;
            let result = &response["result"];
            assert_eq!(
                result["protocolVersion"],
                if version == "unknown" {
                    "2025-11-25"
                } else {
                    version
                }
            );
            assert_eq!(result["instructions"], server_instructions());
        }
        let opened = fixture
            .tool(
                "workspace_open",
                json!({"path":fixture.root,"summary":"Open the fixture to read Agents.md."}),
            )
            .await;
        assert_eq!(opened["isError"], false);
        assert_eq!(
            opened["structuredContent"]["instructions"],
            format!("{} {WORKSPACE_INSTRUCTIONS}", crate::resources::READ_FIRST)
        );
        assert!(opened["structuredContent"]["result"]["id"].is_string());
        let listed = fixture
            .tool(
                "workspace_list",
                json!({"summary":"Find the fixture workspace before reading its guidance."}),
            )
            .await;
        assert_eq!(listed["isError"], false);
        assert_eq!(
            listed["structuredContent"]["instructions"],
            format!("{} {WORKSPACE_INSTRUCTIONS}", crate::resources::READ_FIRST)
        );
        assert!(listed["structuredContent"]["result"].is_array());
        assert_eq!(listed["content"].as_array().unwrap().len(), 1);
        assert!(
            listed["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Result: ok · ")
        );
        assert!(listed["structuredContent"]["time_cost_ms"].is_u64());
    }

    #[tokio::test]
    async fn every_tool_rejects_invalid_summaries_before_dispatch() {
        let fixture = TestApp::new();
        for tool in tool_definitions() {
            let name = tool["name"].as_str().unwrap();
            for summary in [
                Value::Null,
                json!(true),
                json!(42),
                json!([]),
                json!({}),
                json!(""),
                json!(" \n\t"),
                json!("x".repeat(MAX_SUMMARY_CHARS + 1)),
            ] {
                let result = fixture.tool(name, json!({"summary":summary})).await;
                assert_eq!(result["isError"], true, "{name}: {result}");
                assert!(
                    result["structuredContent"]["result"]["error"]
                        .as_str()
                        .unwrap()
                        .contains("summary")
                );
                assert!(result["structuredContent"].get("summary").is_none());
            }
            let missing = fixture.tool(name, json!({})).await;
            assert_eq!(missing["isError"], true);
            assert!(
                missing["structuredContent"]["result"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("summary")
            );
        }
        assert!(fixture.app.disk.lock().unwrap().workspaces.is_empty());
        assert!(fixture.app.disk.lock().unwrap().jobs.is_empty());
    }

    #[tokio::test]
    async fn validation_blocks_file_side_effects_and_preserves_success_and_error_results() {
        let fixture = TestApp::new();
        let opened = fixture
            .tool(
                "workspace_open",
                json!({"path":fixture.root,"summary":"Open the test workspace."}),
            )
            .await;
        let workspace = &opened["structuredContent"]["result"]["id"];
        let mut args = json!({"workspace":workspace,"path":"sentinel.txt","text":"verified"});
        let rejected = fixture.tool("write_file", args.clone()).await;
        assert_eq!(rejected["isError"], true);
        assert!(!fixture.root.join("sentinel.txt").exists());
        args["summary"] = json!("  Save the fixture text for verification.  ");
        let result = fixture.tool("write_file", args).await;
        assert_eq!(result["isError"], false);
        assert!(result["structuredContent"].get("summary").is_none());
        assert!(result["structuredContent"]["time_cost_ms"].is_u64());
        assert_eq!(result["structuredContent"]["result"]["bytes"], 8);
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("sentinel.txt")).unwrap(),
            "verified"
        );
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Result: ok · ")
        );
        let failed = fixture.tool("read_file", json!({"workspace":workspace,"path":"absent.txt","summary":"Read an absent file to verify error handling."})).await;
        assert_eq!(failed["isError"], true);
        assert!(failed["structuredContent"].get("summary").is_none());
        assert!(failed["structuredContent"]["time_cost_ms"].is_u64());
        let error_text = failed["content"][0]["text"].as_str().unwrap();
        let error = failed["structuredContent"]["result"]["error"]
            .as_str()
            .unwrap();
        assert!(!error.is_empty());
        assert!(error_text.starts_with("Result: error · "));
        assert!(error_text.ends_with(error));
    }
    #[tokio::test]
    async fn resource_protocol_and_bridge_errors_are_safe_bootstrap_calls() {
        let fixture = TestApp::new();
        let init = fixture.request("initialize", json!({})).await;
        assert_eq!(
            init["result"]["capabilities"]["resources"]["subscribe"],
            false
        );
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains(crate::resources::INSTRUCTION_URI)
        );
        let protocol = fixture.request("resources/list", json!({})).await;
        let bridge = fixture
            .tool(
                "list_resources",
                json!({"summary":"Discover the server guide."}),
            )
            .await;
        assert_eq!(bridge["structuredContent"]["result"], protocol["result"]);
        assert_eq!(
            fixture.request("resources/templates/list", json!({})).await["result"],
            json!({"resourceTemplates":[]})
        );
        for (params, code) in [
            (json!({}), -32602),
            (json!({"uri":false}), -32602),
            (json!({"uri":"file:///etc/passwd"}), -32002),
        ] {
            assert_eq!(
                fixture.request("resources/read", params.clone()).await["error"]["code"],
                code
            );
            let mut params = params;
            params["summary"] = json!("Verify resource URI validation.");
            assert_eq!(fixture.tool("read_resource", params).await["isError"], true);
        }
        for name in [
            "list_resources",
            "read_resource",
            "workspace_list",
            "read_file",
        ] {
            assert_eq!(tool_annotations(name)["readOnlyHint"], true);
            assert_eq!(tool_annotations(name)["destructiveHint"], false);
            assert_eq!(tool_annotations(name)["openWorldHint"], false);
        }
        assert_eq!(tool_annotations("virtual_pointer")["readOnlyHint"], false);
        assert_eq!(tool_annotations("virtual_pointer")["destructiveHint"], true);
        assert!(fixture.app.disk.lock().unwrap().workspaces.is_empty());
    }
}
