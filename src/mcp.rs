use crate::state::App;
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_SUMMARY_CHARS: usize = 1000;
const WORKSPACE_INSTRUCTIONS: &str = "Before inspecting project files, running commands, editing, or delegating work, read the workspace-root Agents.md with read_file first. If it is absent, try AGENTS.md; if neither exists, report that and continue. Follow has_more/next_offset until the entire guidance file has been read. Read applicable nested Agents.md/AGENTS.md before working in a subdirectory. Opening or listing workspaces and reading guidance are bootstrap steps allowed before other project work.";
const SUMMARY_INSTRUCTIONS: &str = "Every tool call must include summary: a concise, nonblank paragraph (at most 1000 characters) explaining what this specific call will do and why. Describe intent, not an unverified outcome; do not include secrets. The summary is client-provided metadata, not executable input.";

fn server_instructions() -> String {
    format!(
        "Local computer and coding agent. Open a folder with workspace_open or select one with workspace_list, then pass its workspace id to project tools. {WORKSPACE_INSTRUCTIONS} {SUMMARY_INSTRUCTIONS} Commands execute on the host."
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
        json!({"name":"agent_run","parameters":{"type":"object","properties":{"workspace":{"type":"string"},"prompt":{"type":"string"}},"required":["workspace","prompt"]}}),
        json!({"name":"agent_status","parameters":{"type":"object","properties":{"job_id":{"type":"string"}},"required":["job_id"]}})
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
            "inputSchema":d["parameters"]
        })
    }).collect()
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
                .unwrap_or("2025-11-25"),"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"lessagent","version":env!("CARGO_PKG_VERSION")},"instructions":server_instructions()})),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools":tool_definitions()})),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = call(app, params).await;
            let (mut v, is_error) = match result {
                Ok(v) => (v, false),
                Err(e) => (json!({"error":e.to_string()}), true),
            };
            enrich_file_result(name, &mut v);
            let image = v.as_object_mut().and_then(|o| o.remove("image"));
            let mut content = mcp_result_content(name, &arguments, &v, image, is_error);
            let mut structured = json!({"result":v});
            if let Ok(summary) = call_summary(&arguments) {
                content.insert(0, json!({"type":"text","text":format!("Call summary (client-provided intent): {summary}")}));
                structured["summary"] = json!(summary);
            }
            if !is_error && matches!(name, "workspace_open" | "workspace_list") {
                content.push(json!({"type":"text","text":WORKSPACE_INSTRUCTIONS}));
                structured["instructions"] = json!(WORKSPACE_INSTRUCTIONS);
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
        "computer" => (
            "Observe and control application windows with screenshots, pointer actions, typing, keys, drags, and scrolling.",
            "Use it for GUI tasks that cannot be completed reliably through files or command-line tools.",
            "On macOS use browser_open (or computer action browser_open with url) to create an isolated background Chrome window; then pass its window_id, pid and screenshot dimensions for input. Unmanaged Chrome input is rejected instead of sharing physical button state. For other native apps call windows, select window_id and pid, and screenshot that target first. macOS is background-only and never moves or falls back to the main pointer; successful input returns a fresh image observation. Linux supports desktop mode.",
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
        "agent_run" => (
            "Start a persistent autonomous Lessagent job in a workspace.",
            "Use it to delegate a multi-step project task to the configured model/provider.",
            "Pass workspace and a concrete prompt, then use agent_status with the returned job_id to inspect progress and output.",
        ),
        "agent_status" => (
            "Read the current status and output of a persistent Lessagent job.",
            "Use it to inspect the result of a job started with agent_run.",
            "Pass the job_id returned by agent_run and check its status/output before deciding the next action.",
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
        "Summary: {summary}\n\nPurpose: {purpose}\n\nHow: {how}\n\nCall summary: {SUMMARY_INSTRUCTIONS}\n\nRead first: {WORKSPACE_INSTRUCTIONS}"
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
    name: &str,
    arguments: &Value,
    value: &Value,
    image: Option<Value>,
    is_error: bool,
) -> Vec<Value> {
    let mut content = Vec::new();
    let text = if is_error {
        format!(
            "Error: {}",
            value["error"].as_str().unwrap_or("tool call failed")
        )
    } else {
        match name {
            "shell" | "bash" | "python" | "terminal_read" | "terminal_write" | "terminal_stop" => {
                let output = value["output"].as_str().unwrap_or_default();
                let shown = if output.is_empty() {
                    "[no command output]"
                } else {
                    output
                };
                let terminal = value["terminal_id"].as_str().unwrap_or("unknown");
                let exited = value["exited"]
                    .as_bool()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into());
                let exit_code = value["exit_code"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "running".into());
                format!(
                    "{shown}\n\nTerminal: {terminal} · exited: {exited} · exit code: {exit_code}"
                )
            }
            "read_file" if value["text"].is_string() => {
                let path = arguments["path"].as_str().unwrap_or("unknown");
                let offset = value["offset"].as_u64().unwrap_or(0);
                let returned = value["returned_bytes"].as_u64().unwrap_or(0);
                let total = value["total_bytes"].as_u64().unwrap_or(0);
                let next = value["next_offset"].as_u64().unwrap_or(total);
                let more = value["has_more"].as_bool().unwrap_or(false);
                let paging = if more {
                    format!(" More remains; continue with offset={next}.")
                } else {
                    String::new()
                };
                format!(
                    "File excerpt: {path} · bytes {offset}..{} of {total}.{}\n\n{}",
                    offset + returned,
                    paging,
                    value["text"].as_str().unwrap_or_default()
                )
            }
            "read_file" => {
                format!(
                    "Image file: {}",
                    arguments["path"].as_str().unwrap_or("unknown")
                )
            }
            "write_image" => format!(
                "Saved image: {} · {}×{} · {} bytes",
                value["path"].as_str().unwrap_or("unknown"),
                value["width"].as_u64().unwrap_or(0),
                value["height"].as_u64().unwrap_or(0),
                value["bytes"].as_u64().unwrap_or(0)
            ),
            "computer" | "browser_open" if image.is_some() => {
                let mut meta = value.clone();
                if let Some(object) = meta.as_object_mut() {
                    object.remove("output");
                }
                format!(
                    "Computer observation:\n{}",
                    serde_json::to_string_pretty(&meta).unwrap_or_else(|_| "{}".into())
                )
            }
            _ => serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        }
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
    match name {
        "workspace_open" => Ok(serde_json::to_value(
            app.open_workspace(std::path::Path::new(crate::tools::string(a, "path")?))?,
        )?),
        "workspace_list" => Ok(json!(app.disk.lock().unwrap().workspaces)),
        "agent_run" => Ok(
            json!({"job_id":crate::agent::start(app,crate::tools::string(a,"workspace")?,crate::tools::string(a,"prompt")?)?}),
        ),
        "agent_status" => {
            let id = crate::tools::string(a, "job_id")?;
            Ok(json!(
                app.disk
                    .lock()
                    .unwrap()
                    .jobs
                    .iter()
                    .find(|j| j.id == id)
                    .ok_or_else(|| crate::err("Job not found"))?
            ))
        }
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
            WORKSPACE_INSTRUCTIONS
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
            WORKSPACE_INSTRUCTIONS
        );
        assert!(listed["structuredContent"]["result"].is_array());
        assert_eq!(
            listed["content"].as_array().unwrap().last().unwrap()["text"],
            WORKSPACE_INSTRUCTIONS
        );
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
        assert_eq!(
            result["structuredContent"]["summary"],
            "Save the fixture text for verification."
        );
        assert_eq!(result["structuredContent"]["result"]["bytes"], 8);
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("sentinel.txt")).unwrap(),
            "verified"
        );
        assert_eq!(
            result["content"][0]["text"],
            "Call summary (client-provided intent): Save the fixture text for verification."
        );
        let failed = fixture.tool("read_file", json!({"workspace":workspace,"path":"absent.txt","summary":"Read an absent file to verify error handling."})).await;
        assert_eq!(failed["isError"], true);
        assert_eq!(
            failed["structuredContent"]["summary"],
            "Read an absent file to verify error handling."
        );
        assert!(
            failed["content"][1]["text"]
                .as_str()
                .unwrap()
                .starts_with("Error: ")
        );
    }
}
