use crate::state::App;
use base64::Engine;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Instant};

const MAX_SUMMARY_CHARS: usize = 500;
const MAX_AGENT_CHARS: usize = 200;
const MAX_MODEL_CHARS: usize = 200;
const MAX_MAIN_TASK_CHARS: usize = 500;
const MAX_CURRENT_TASK_CHARS: usize = 300;
const MAX_CURRENT_TIMESTAMP_CHARS: usize = 120;
const MAX_TRANSFER_FILES: usize = 64;
const MAX_TRANSFER_TOTAL_BYTES: usize = 24 * 1024 * 1024;
const WORKSPACE_INSTRUCTIONS: &str = "Before project work, read the entire workspace-root Agents.md with bash or python; try AGENTS.md if absent, report if neither exists, and read applicable nested guidance before editing a subdirectory.";

fn server_instructions() -> String {
    format!(
        "Local computer and coding agent. {} Open a folder with workspace_open or select one with workspace_list. Read instruction.md for the MCP metadata contract, browser_open/app_open workflow, and project workflow. {WORKSPACE_INSTRUCTIONS} Commands execute on the host.",
        crate::resources::READ_FIRST
    )
}

/// Apply the MCP-only contract centrally, including tools without a workspace.
/// Internal agent/CLI tool definitions deliberately keep their existing schema.
fn tool_definitions() -> Vec<Value> {
    let mut defs = crate::tools::definitions();
    defs.extend([
        json!({"name":"get_files","description":"Read multiple workspace files and return them as MCP media/resource content blocks.","parameters":{"type":"object","properties":{"paths":{"type":"array","minItems":1,"maxItems":MAX_TRANSFER_FILES,"items":{"type":"string","minLength":1}}},"required":["paths"],"additionalProperties":false}}),
        json!({"name":"send_files","description":"Receive multiple base64 files and save them atomically inside the workspace.","parameters":{"type":"object","properties":{"files":{"type":"array","minItems":1,"maxItems":MAX_TRANSFER_FILES,"items":{"type":"object","properties":{"path":{"type":"string","minLength":1},"mimeType":{"type":"string","minLength":1,"maxLength":200},"data":{"type":"string","minLength":1}},"required":["path","data"],"additionalProperties":false}}},"required":["files"],"additionalProperties":false}}),
    ]);
    defs.retain(|d| !matches!(d["name"].as_str(), Some("read_file" | "write_file")));
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
        json!({"name":"read_resource","parameters":{"type":"object","properties":{"uri":{"type":"string","description":"Exact URI from list_resources; use lessagent://server/instruction.md for host information and server guidance."}},"required":["uri"],"additionalProperties":false}}),
        json!({"name":"wait_n","parameters":{"type":"object","properties":{"seconds":{"type":"number","minimum":0,"maximum":3600,"description":"Seconds to pause before returning; fractional seconds are allowed."}},"required":["seconds"],"additionalProperties":false}})
    ]);
    defs.into_iter().map(|mut d| {
        add_required_parameter_first(&mut d["parameters"], "summary", json!({
            "type":"string", "minLength":1, "maxLength":MAX_SUMMARY_CHARS, "pattern":r"\S",
            "description":"State what is done or verified, then use a blank line (\n\n) before what this tool call will do and why; max 500."
        }));
        add_required_parameter(&mut d["parameters"], "agent", json!({
            "type":"string", "minLength":1, "maxLength":MAX_AGENT_CHARS, "pattern":r"\S",
            "description":"Calling agent/client."
        }));
        add_required_parameter(&mut d["parameters"], "model", json!({
            "type":"string", "minLength":1, "maxLength":MAX_MODEL_CHARS, "pattern":r"\S",
            "description":"Calling model."
        }));
        add_required_parameter(&mut d["parameters"], "main_task", json!({
            "type":"string", "minLength":1, "maxLength":MAX_MAIN_TASK_CHARS, "pattern":r"\S",
            "description":"Overall task name."
        }));
        add_required_parameter(&mut d["parameters"], "current_task", json!({
            "type":"string", "minLength":1, "maxLength":MAX_CURRENT_TASK_CHARS, "pattern":r"\S",
            "description":"Current step or subtask."
        }));
        add_required_parameter(&mut d["parameters"], "progress", json!({
            "type":"integer", "minimum":0, "maximum":100,
            "description":"Overall task progress, 0-100."
        }));
        add_required_parameter(&mut d["parameters"], "quality", json!({
            "type":"integer", "minimum":0, "maximum":100,
            "description":"Verified quality/confidence, 0-100."
        }));
        add_required_parameter(&mut d["parameters"], "current_timestamp", json!({
            "type":"string", "minLength":1, "maxLength":MAX_CURRENT_TIMESTAMP_CHARS, "pattern":r"\S",
            "description":"Caller timestamp; ISO 8601 preferred."
        }));
        let name = d["name"].as_str().unwrap_or_default();
        json!({
            "name":name,
            "description":mcp_tool_description(name, d["description"].as_str().unwrap_or_default()),
            "inputSchema":d["parameters"],
            "outputSchema":mcp_output_schema(name),
            "annotations":tool_annotations(name)
        })
    }).collect()
}

/// Schema for the structuredContent envelope returned by tools/call.
/// Keep the envelope stable while allowing each tool's result payload to evolve.
fn mcp_output_schema(name: &str) -> Value {
    let (result_schema, result_description) = match name {
        "bash" | "shell" | "python" => (
            json!({"type":"object"}),
            "Command result containing terminal_id, output, exited, and exit_code.",
        ),
        "terminal_read" | "terminal_write" | "terminal_stop" => (
            json!({"type":"object"}),
            "Terminal snapshot containing terminal_id, output, exited, and exit_code.",
        ),
        "workspace_list" => (json!({"type":"array"}), "Array of saved workspaces."),
        _ => (json!({}), "Tool-specific result payload."),
    };
    let mut result_schema = result_schema;
    result_schema["description"] = json!(result_description);
    json!({
        "type":"object",
        "properties":{
            "result":result_schema,
            "time_cost_ms":{"type":"integer","minimum":0,"description":"Server execution time in milliseconds."},
            "instructions":{"type":"string","description":"Bootstrap guidance returned by workspace_open/list."}
        },
        "required":["result","time_cost_ms"],
        "additionalProperties":false
    })
}

fn tool_annotations(name: &str) -> Value {
    let read_only = matches!(
        name,
        "list_files"
            | "get_files"
            | "workspace_list"
            | "terminal_read"
            | "list_resources"
            | "read_resource"
            | "get_screenshot"
            | "list_windows"
            | "wait_n"
    );
    let non_destructive = read_only || matches!(name, "workspace_open");
    let closed_world = read_only || matches!(name, "write_image" | "send_files" | "terminal_stop");
    json!({"readOnlyHint":read_only, "destructiveHint":!non_destructive,
        "idempotentHint":read_only || matches!(name, "workspace_open" | "write_image" | "send_files" | "terminal_stop"),
        "openWorldHint":!closed_world})
}

fn add_required_parameter_first(schema: &mut Value, name: &str, property: Value) {
    if !schema["properties"].is_object() {
        schema["properties"] = json!({});
    }
    let properties = schema["properties"].as_object_mut().unwrap();
    properties.remove(name);
    let old = std::mem::take(properties);
    let mut ordered = serde_json::Map::new();
    ordered.insert(name.to_owned(), property);
    for (key, value) in old {
        ordered.insert(key, value);
    }
    *properties = ordered;
    if !schema["required"].is_array() {
        schema["required"] = json!([]);
    }
    let required = schema["required"].as_array_mut().unwrap();
    required.retain(|value| value != name);
    required.insert(0, json!(name));
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

fn call_context_text<'a>(
    arguments: &'a Value,
    key: &str,
    max_chars: usize,
) -> crate::Result<&'a str> {
    let value = arguments[key].as_str().ok_or_else(|| {
        crate::err(format!(
            "Missing or invalid {key}: include a nonblank string for MCP call context"
        ))
    })?;
    if value.trim().is_empty() {
        return Err(crate::err(format!(
            "{key} must contain non-whitespace text"
        )));
    }
    if value.chars().count() > max_chars {
        return Err(crate::err(format!(
            "{key} must be at most {max_chars} characters"
        )));
    }
    Ok(value.trim())
}

fn call_score(arguments: &Value, key: &str) -> crate::Result<u64> {
    arguments[key]
        .as_u64()
        .filter(|value| *value <= 100)
        .ok_or_else(|| crate::err(format!("Missing or invalid {key}: expected integer 0-100")))
}

fn call_metadata(arguments: &Value) -> crate::Result<Value> {
    let summary = call_summary(arguments)?;
    let agent = call_context_text(arguments, "agent", MAX_AGENT_CHARS)?;
    let model = call_context_text(arguments, "model", MAX_MODEL_CHARS)?;
    let main_task = call_context_text(arguments, "main_task", MAX_MAIN_TASK_CHARS)?;
    let current_task = call_context_text(arguments, "current_task", MAX_CURRENT_TASK_CHARS)?;
    let progress = call_score(arguments, "progress")?;
    let quality = call_score(arguments, "quality")?;
    let current_timestamp =
        call_context_text(arguments, "current_timestamp", MAX_CURRENT_TIMESTAMP_CHARS)?;
    Ok(json!({
        "source":"MCP",
        "summary":summary,
        "agent":agent,
        "model":model,
        "main_task":main_task,
        "current_task":current_task,
        "progress":progress,
        "quality":quality,
        "current_timestamp":current_timestamp
    }))
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
            let image = v.as_object_mut().and_then(|o| o.remove("image"));
            let extra_content = v
                .as_object_mut()
                .and_then(|o| o.remove("_mcp_content"))
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default();
            let content = mcp_result_content(&v, image, extra_content, is_error, time_cost_ms);
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
        "browser_open" => (
            "Open a new background Chrome window in the existing persistent managed profile by default, with a default size of 1000 by 600 logical points.",
            "Use it to create another controlled window while preserving cookies/storage from Lessagent's existing managed Chrome profile and leaving the user's normal browser and physical pointer untouched.",
            "Pass workspace, summary and an HTTP(S) url; include a purpose query parameter describing why this window exists and the model name, using ?purpose=texttodescribepurposeofthiswindow_by_modelname (or &purpose=... when the URL already has a query string, with the value URL-encoded). Optional width/height must not exceed the current primary display's logical resolution. browser_open creates a new window but reuses the persistent Lessagent-managed user-data directory by default, and reuses its live Chrome process when available; it creates a new managed profile only when none exists. Reuse returned window_id/pid and image width/height with virtual_pointer/virtual_keyboard. Screenshots default to project output/computer/.",
        ),
        "app_open" => (
            "Open or reuse a native application without requesting foreground focus.",
            "Use it for native apps such as Blender; Chrome must use browser_open instead.",
            "Pass app (name, bundle ID or .app path). Keep the app's normal/default profile or session. new_instance defaults true; prefer false for ordinary work to reuse one unambiguous existing window, or launch normally when not running. A new instance does not request a fresh profile. Blender launches with --no-window-focus. Returns IDs and an automatic screenshot. Other apps may ignore nonactivation; inspect focus_changed. Never restarts an existing app.",
        ),
        "get_screenshot" => (
            "Capture a standalone read-only screenshot.",
            "Use it for the initial window view or recovery from screenshot_error, with the same standalone screenshot behavior and metadata returned after virtual input.",
            "Pass workspace and optional capture_path/show_pointer. For a macOS window pass exact window_id and pid from list_windows. Image pixel dimensions are authoritative. Stage Manager can require a perspective-corrected low-resolution thumbnail; inspect capture_quality. Capture never activates the app.",
        ),
        "virtual_pointer" => (
            "Control a standalone background virtual pointer.",
            "Use move, click, drag or scroll on the selected window without moving the physical pointer.",
            "Pass action, window_id/pid on macOS, image pixel coordinates and screen_width/screen_height. Drag accepts a continuous path, button:left/right/middle, and optional modifiers:[shift,ctrl,alt,cmd]. Blender middle drag orbits; Shift+middle pans. Every input waits three seconds, then returns a fresh screenshot. Chrome requires browser_open; native windows use exact-ID geometry. Never replay input solely because capture failed.",
        ),
        "virtual_keyboard" => (
            "Type text or press a key chord in a background window.",
            "Use it after selecting the intended input with virtual_pointer. Keyboard input is separate from pointer and screenshot tools.",
            "Pass action:type with text, or action:key with key (such as cmd+a), plus window_id/pid on macOS. Never supply both text and key. For Blender, move/click in the intended editor first; keyboard calls restore that window’s last virtual position before typing so modal transforms cannot lose editor context. After a resize/helper restart, select the editor again. Keys include numpad0..9, numpaddecimal, minus and period. Returns a fresh automatic screenshot after three seconds. A non-activating top-right key notice shows down/up/click and hides after five seconds; text content is not echoed. No global-input fallback.",
        ),
        "list_windows" => (
            "List existing windows without activation.",
            "Use it to select the exact existing window and owner for background control.",
            "Pass workspace, then reuse window_id and pid with get_screenshot, virtual_pointer and virtual_keyboard. Window listing is read-only; screenshot the target before choosing coordinates.",
        ),
        "shell" => (
            "Run Bash in a visible persistent workspace terminal.",
            "Use it for interactive or long-running shell work that should remain attached to a PTY.",
            "Pass workspace and command. If it keeps running, use terminal_read for new output, terminal_write for input, and terminal_stop to end it.",
        ),
        "bash" => (
            "Run Bash code in a workspace PTY.",
            "Use it for builds, tests, searches, scripts, and other host shell commands.",
            "Pass workspace and command, optionally wait_ms. Read output and exit status from the response; use terminal_read/write/stop if the process is still running. The PTY receives LESSAGENT_BIN, LESSAGENT_PORT, LESSAGENT_DATA_DIR and LESSAGENT_WORKSPACE_ID for workspace/server context. Open macOS apps in the background by default with open -g as documented in instruction.md.",
        ),
        "python" => (
            "Run Python 3 code in a new unbuffered workspace PTY.",
            "Use it for local scripting, data processing, checks, and small automation tasks.",
            "Pass workspace and code, optionally wait_ms. Read output and exit status from the response; use terminal_read/write/stop if it remains active.",
        ),
        "wait_n" => (
            "Pause for a caller-selected number of seconds and then return.",
            "Use it for a simple rest or delay without running shell commands or requiring a workspace.",
            "Pass seconds from 0 through 3600; fractional seconds are allowed. The normal MCP response includes time_cost_ms so the caller can see the measured call duration.",
        ),
        "write_image" => (
            "Save an MCP image content block into the workspace and return the image.",
            "Use it to persist generated or transferred PNG, JPEG, GIF, or WebP images as workspace artifacts.",
            "Pass workspace, a workspace-relative path, and an image block whose mime type matches the file extension.",
        ),
        "get_files" => (
            "Get multiple workspace files in one MCP call.",
            "Use it to transfer images, audio, video, documents, archives, 3D files, and other binary files from the workspace to the MCP client.",
            "Pass workspace and paths (1-64 relative paths). Images use native MCP image blocks, audio uses native audio blocks, and all other types use embedded resource blobs. The batch is limited to 24 MiB raw data.",
        ),
        "send_files" => (
            "Send multiple files into the workspace in one MCP call.",
            "Use it to transfer images, audio, video, documents, archives, 3D files, and arbitrary binary files from the MCP client to Lessagent.",
            "Pass workspace and files (1-64 items), each with path, base64 data, and optional mimeType. All files are decoded and path-checked before writes begin; the batch is limited to 24 MiB raw data.",
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
        "list_files" => (
            "List workspace files with context token estimates while respecting ignore rules.",
            "Use it to discover the project layout and choose which files need inspection.",
            "Pass workspace. Use bash or python for targeted file reads after discovery.",
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
        "Summary: {summary}\n\nPurpose: {purpose}\n\nHow: {how}\n\nRead first: {}",
        crate::resources::READ_FIRST
    )
}

fn mcp_result_content(
    value: &Value,
    image: Option<Value>,
    mut extra_content: Vec<Value>,
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
    content.append(&mut extra_content);
    content
}

fn transfer_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "blend" => "application/x-blender",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "obj" => "model/obj",
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "xml" => "application/xml",
        "txt" | "md" | "rs" | "py" | "sh" | "toml" | "yaml" | "yml" | "csv" => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

fn transfer_kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime.starts_with("video/") {
        "video"
    } else {
        "file"
    }
}

fn transfer_resource_uri(path: &str) -> String {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path.as_bytes());
    format!("lessagent://workspace-file/{encoded}")
}

fn transfer_content_block(path: &str, bytes: &[u8], mime: &str) -> (Value, &'static str) {
    let file_meta = json!({"path":path,"mimeType":mime,"bytes":bytes.len()});
    if matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        let mut image_result = json!({});
        if crate::image_content::attach(&mut image_result, bytes, mime).is_ok() {
            let mut block = crate::image_content::mcp_block(&image_result["image"]);
            if !block["_meta"].is_object() {
                block["_meta"] = json!({});
            }
            block["_meta"]["lessagent/file"] = file_meta;
            return (block, "image");
        }
    }
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    if mime.starts_with("audio/") {
        return (
            json!({"type":"audio","mimeType":mime,"data":data,"_meta":{"lessagent/file":file_meta}}),
            "audio",
        );
    }
    (
        json!({"type":"resource","resource":{"uri":transfer_resource_uri(path),"mimeType":mime,"blob":data},"_meta":{"lessagent/file":file_meta}}),
        "resource",
    )
}

fn validate_transfer_mime(value: Option<&Value>, path: &Path) -> crate::Result<String> {
    let Some(value) = value else {
        return Ok(transfer_mime(path).to_owned());
    };
    let mime = value
        .as_str()
        .ok_or_else(|| crate::err("mimeType must be a string when provided"))?
        .trim();
    if mime.is_empty()
        || mime.chars().count() > 200
        || !mime.contains('/')
        || mime.chars().any(char::is_control)
    {
        return Err(crate::err("mimeType must be a valid nonblank MIME type"));
    }
    Ok(mime.to_owned())
}

fn execute_transfer_tool(
    app: Arc<App>,
    workspace: &str,
    name: &str,
    args: &Value,
    metadata: Value,
) -> crate::Result<Value> {
    let w = app.workspace(workspace)?;
    let mut details = crate::tools::tool_log_details(name, args);
    if let (Some(target), Some(metadata)) = (details.as_object_mut(), metadata.as_object()) {
        for (key, value) in metadata {
            target.insert(key.clone(), value.clone());
        }
    }
    let log_id = app.log_with_details_id("tool", &format!("{name} · {}", w.name), Some(details));
    let result = (|| -> crate::Result<Value> {
        match name {
            "get_files" => {
                let paths = args["paths"].as_array().ok_or_else(|| {
                    crate::err("paths must be an array of 1-64 workspace-relative paths")
                })?;
                if paths.is_empty() || paths.len() > MAX_TRANSFER_FILES {
                    return Err(crate::err("paths must contain 1-64 files"));
                }
                let mut seen = std::collections::HashSet::new();
                let mut total_bytes = 0usize;
                let mut files = Vec::with_capacity(paths.len());
                let mut content = Vec::with_capacity(paths.len());
                for value in paths {
                    let path = value
                        .as_str()
                        .filter(|path| !path.trim().is_empty())
                        .ok_or_else(|| {
                            crate::err("Every get_files path must be a nonblank string")
                        })?;
                    let resolved = crate::context::resolve(&w.path, path, false)?;
                    let canonical = resolved.canonicalize()?;
                    if !seen.insert(canonical) {
                        return Err(crate::err(format!(
                            "Duplicate file path in get_files: {path}"
                        )));
                    }
                    let metadata = resolved.metadata()?;
                    if !metadata.is_file() {
                        return Err(crate::err(format!("Not a regular file: {path}")));
                    }
                    let size = usize::try_from(metadata.len())
                        .map_err(|_| crate::err(format!("File is too large: {path}")))?;
                    total_bytes = total_bytes
                        .checked_add(size)
                        .ok_or_else(|| crate::err("File batch size overflow"))?;
                    if total_bytes > MAX_TRANSFER_TOTAL_BYTES {
                        return Err(crate::err("get_files batch exceeds 24 MiB raw-data limit"));
                    }
                    let bytes = std::fs::read(&resolved)?;
                    let mime = transfer_mime(&resolved);
                    let (block, content_type) = transfer_content_block(path, &bytes, mime);
                    files.push(json!({"path":path,"mimeType":mime,"bytes":bytes.len(),"kind":transfer_kind(mime),"content_type":content_type}));
                    content.push(block);
                }
                let count = files.len();
                Ok(
                    json!({"files":files,"count":count,"total_bytes":total_bytes,"_mcp_content":content}),
                )
            }
            "send_files" => {
                let files = args["files"]
                    .as_array()
                    .ok_or_else(|| crate::err("files must be an array of 1-64 file objects"))?;
                if files.is_empty() || files.len() > MAX_TRANSFER_FILES {
                    return Err(crate::err("files must contain 1-64 items"));
                }
                let mut seen = std::collections::HashSet::new();
                let mut total_bytes = 0usize;
                let mut pending = Vec::with_capacity(files.len());
                for item in files {
                    let object = item
                        .as_object()
                        .ok_or_else(|| crate::err("Every send_files item must be an object"))?;
                    let path = object
                        .get("path")
                        .and_then(Value::as_str)
                        .filter(|path| !path.trim().is_empty())
                        .ok_or_else(|| crate::err("Every send_files item needs a nonblank path"))?;
                    let resolved = crate::context::resolve(&w.path, path, true)?;
                    if !seen.insert(resolved.clone()) {
                        return Err(crate::err(format!(
                            "Duplicate file path in send_files: {path}"
                        )));
                    }
                    let data = object
                        .get("data")
                        .and_then(Value::as_str)
                        .filter(|data| !data.is_empty())
                        .ok_or_else(|| {
                            crate::err("Every send_files item needs nonblank base64 data")
                        })?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .map_err(|_| crate::err(format!("Invalid base64 data for {path}")))?;
                    total_bytes = total_bytes
                        .checked_add(bytes.len())
                        .ok_or_else(|| crate::err("File batch size overflow"))?;
                    if total_bytes > MAX_TRANSFER_TOTAL_BYTES {
                        return Err(crate::err("send_files batch exceeds 24 MiB raw-data limit"));
                    }
                    let mime = validate_transfer_mime(object.get("mimeType"), &resolved)?;
                    pending.push((resolved, path.to_owned(), mime, bytes));
                }
                let mut written = Vec::with_capacity(pending.len());
                for (resolved, path, mime, bytes) in pending {
                    crate::state::atomic_write(&resolved, &bytes)?;
                    written.push(json!({"path":path,"mimeType":mime,"bytes":bytes.len(),"kind":transfer_kind(&mime)}));
                }
                let count = written.len();
                Ok(json!({"files":written,"count":count,"total_bytes":total_bytes}))
            }
            _ => Err(crate::err(format!("Unknown transfer tool: {name}"))),
        }
    })();
    let patch = match &result {
        Ok(value) => json!({"output":crate::tools::tool_log_output(name, value)}),
        Err(error) => json!({"output":{"error":crate::clip(&error.to_string(), 16_000)}}),
    };
    app.update_log_details(&log_id, patch);
    result
}

async fn call(app: Arc<App>, p: &Value) -> crate::Result<Value> {
    let name = crate::tools::string(p, "name")?;
    let mut arguments = p.get("arguments").cloned().unwrap_or_else(|| json!({}));
    // Validate all request-only observability metadata before any side effects.
    let metadata = call_metadata(&arguments)?;
    // Removed public MCP-only surfaces are rejected before workspace dispatch.
    // Their internal backend capabilities remain available to the normal Lessagent runtime.
    if name == "computer" {
        return Err(crate::err(
            "Unknown tool: computer; use the standalone GUI tools instead",
        ));
    }
    if matches!(
        name,
        "agent_run" | "agent_status" | "read_file" | "write_file"
    ) {
        return Err(crate::err(format!(
            "Unknown tool: {name}; use bash or python instead"
        )));
    }
    // Metadata must never reach shell code, delegated prompts, or GUI input.
    for key in [
        "summary",
        "agent",
        "model",
        "main_task",
        "current_task",
        "progress",
        "quality",
        "current_timestamp",
    ] {
        arguments.as_object_mut().unwrap().remove(key);
    }
    let a = &arguments;
    if matches!(name, "get_files" | "send_files") {
        return execute_transfer_tool(
            app,
            crate::tools::string(a, "workspace")?,
            name,
            a,
            metadata,
        );
    }
    if matches!(
        name,
        "list_resources" | "read_resource" | "workspace_open" | "workspace_list" | "wait_n"
    ) {
        let mut details = crate::tools::tool_log_details(name, a);
        if let (Some(target), Some(metadata)) = (details.as_object_mut(), metadata.as_object()) {
            for (key, value) in metadata {
                target.insert(key.clone(), value.clone());
            }
        }
        let log_id = app.log_with_details_id("tool", &format!("{name} · MCP"), Some(details));
        let result = match name {
            "list_resources" => {
                crate::resources::list(a).map_err(|(_, message)| crate::err(message))
            }
            "read_resource" => crate::resources::read(app.clone(), a)
                .await
                .map_err(|(_, message)| crate::err(message)),
            "workspace_open" => crate::tools::string(a, "path")
                .and_then(|path| app.open_workspace(std::path::Path::new(path)))
                .and_then(|workspace| serde_json::to_value(workspace).map_err(crate::Error::from)),
            "workspace_list" => Ok(json!(app.disk.lock().unwrap().workspaces)),
            "wait_n" => {
                let seconds = a["seconds"]
                    .as_f64()
                    .ok_or_else(|| crate::err("seconds must be a number between 0 and 3600"))?;
                if !seconds.is_finite() || !(0.0..=3600.0).contains(&seconds) {
                    Err(crate::err("seconds must be a number between 0 and 3600"))
                } else {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
                    Ok(json!({"waited_seconds":seconds}))
                }
            }
            _ => unreachable!(),
        };
        let patch = match &result {
            Ok(value) => json!({"output":crate::tools::tool_log_output(name, value)}),
            Err(error) => json!({"output":{"error":crate::clip(&error.to_string(), 16_000)}}),
        };
        app.update_log_details(&log_id, patch);
        result
    } else {
        crate::tools::execute_mcp(
            app,
            crate::tools::string(a, "workspace")?,
            name,
            a,
            metadata,
        )
        .await
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

    fn with_context(mut arguments: Value, summary: &str) -> Value {
        if !arguments.is_object() {
            arguments = json!({});
        }
        let object = arguments.as_object_mut().unwrap();
        object.insert("summary".into(), json!(summary));
        object.insert("agent".into(), json!("mcp-unit-test"));
        object.insert("model".into(), json!("test-model"));
        object.insert("main_task".into(), json!("Verify MCP metadata"));
        object.insert("current_task".into(), json!("Run focused unit check"));
        object.insert("progress".into(), json!(60));
        object.insert("quality".into(), json!(95));
        object.insert(
            "current_timestamp".into(),
            json!("2026-09-12T08:00:00-07:00"),
        );
        arguments
    }

    #[test]
    fn public_mcp_surface_is_small_and_metadata_is_standalone() {
        let definitions = tool_definitions();
        let names: std::collections::HashSet<_> = definitions
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for removed in [
            "read_file",
            "write_file",
            "computer",
            "agent_run",
            "agent_status",
        ] {
            assert!(!names.contains(removed), "{removed} was advertised");
        }
        for required in [
            "browser_open",
            "app_open",
            "bash",
            "python",
            "shell",
            "list_files",
            "get_files",
            "send_files",
            "get_screenshot",
            "virtual_pointer",
            "virtual_keyboard",
            "list_windows",
            "workspace_open",
            "workspace_list",
            "list_resources",
            "read_resource",
            "wait_n",
        ] {
            assert!(names.contains(required), "missing public tool {required}");
        }
        for tool in &definitions {
            let schema = &tool["inputSchema"];
            let properties = schema["properties"].as_object().unwrap();
            let required = schema["required"].as_array().unwrap();
            for (field, max_chars) in [
                ("summary", MAX_SUMMARY_CHARS),
                ("agent", MAX_AGENT_CHARS),
                ("model", MAX_MODEL_CHARS),
                ("main_task", MAX_MAIN_TASK_CHARS),
                ("current_task", MAX_CURRENT_TASK_CHARS),
                ("current_timestamp", MAX_CURRENT_TIMESTAMP_CHARS),
            ] {
                let property = &properties[field];
                assert_eq!(property["type"], "string", "{} {field}", tool["name"]);
                assert_eq!(property["minLength"], 1);
                assert_eq!(property["maxLength"], max_chars);
                assert_eq!(property["pattern"], r"\S");
                assert_eq!(required.iter().filter(|value| **value == field).count(), 1);
            }
            for field in ["progress", "quality"] {
                let property = &properties[field];
                assert_eq!(property["type"], "integer");
                assert_eq!(property["minimum"], 0);
                assert_eq!(property["maximum"], 100);
                assert_eq!(required.iter().filter(|value| **value == field).count(), 1);
            }
            let keys: Vec<_> = properties.keys().map(String::as_str).collect();
            assert_eq!(keys.first(), Some(&"summary"), "{}", tool["name"]);
            assert_eq!(keys.last(), Some(&"current_timestamp"), "{}", tool["name"]);
            assert_eq!(required.first(), Some(&json!("summary")));
            assert_eq!(required.last(), Some(&json!("current_timestamp")));
            assert!(
                properties["summary"]["description"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .count()
                    < 50
            );
            let description = tool["description"].as_str().unwrap();
            for marker in ["Summary: ", "\n\nPurpose: ", "\n\nHow: ", "Read first:"] {
                assert!(description.contains(marker), "{}: {marker}", tool["name"]);
            }
            assert!(!description.contains("Progress N/100"));
            assert!(!description.contains("Main task:"));
        }
        for tool in crate::tools::definitions() {
            for field in [
                "summary",
                "agent",
                "model",
                "main_task",
                "current_task",
                "progress",
                "quality",
                "current_timestamp",
            ] {
                assert!(tool["parameters"]["properties"].get(field).is_none());
            }
        }
    }

    #[tokio::test]
    async fn removed_mcp_tools_are_rejected_with_replacements() {
        let fixture = TestApp::new();
        for (name, expected) in [
            ("read_file", "bash or python"),
            ("write_file", "bash or python"),
            ("agent_run", "bash or python"),
            ("agent_status", "bash or python"),
        ] {
            let result = fixture
                .tool(
                    name,
                    with_context(json!({}), "Mapped tools; reject old surface"),
                )
                .await;
            assert_eq!(result["isError"], true, "{name}: {result}");
            assert!(
                result["structuredContent"]["result"]["error"]
                    .as_str()
                    .unwrap()
                    .contains(expected)
            );
        }
        let computer = fixture
            .tool(
                "computer",
                with_context(json!({}), "Mapped tools; reject aggregate"),
            )
            .await;
        assert_eq!(computer["isError"], true);
        assert!(
            computer["structuredContent"]["result"]["error"]
                .as_str()
                .unwrap()
                .contains("standalone GUI tools")
        );
    }

    #[tokio::test]
    async fn restored_app_open_keeps_metadata_permissions_and_argument_guards() {
        let definitions = tool_definitions();
        let launchers: Vec<_> = definitions
            .iter()
            .filter(|tool| tool["name"] == "app_open")
            .collect();
        assert_eq!(launchers.len(), 1);
        let schema = &launchers[0]["inputSchema"];
        assert_eq!(schema["properties"]["summary"]["maxLength"], 500);
        assert_eq!(schema["properties"]["new_instance"]["default"], true);
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"].get("action").is_none());
        assert_eq!(launchers[0]["annotations"]["readOnlyHint"], false);

        let fixture = TestApp::new();
        let workspace = fixture.app.open_workspace(&fixture.root).unwrap().id;
        let summary = "雪".repeat(500);
        let disabled = fixture
            .tool(
                "app_open",
                with_context(
                    json!({"workspace":workspace,"app":"Blender","new_instance":false}),
                    &summary,
                ),
            )
            .await;
        assert_eq!(disabled["isError"], true);
        assert!(
            disabled["structuredContent"]["result"]["error"]
                .as_str()
                .unwrap()
                .contains("Enable computer control")
        );
        let logs = fixture.app.disk.lock().unwrap().logs.clone();
        let details = &logs.last().unwrap()["details"];
        assert_eq!(details["summary"], summary);
        assert_eq!(details["arguments"]["new_instance"], false);
        assert!(details["arguments"].get("summary").is_none());

        // Invalid launcher input must fail before the platform helper is called.
        fixture.app.disk.lock().unwrap().settings.computer_enabled = true;
        for (mut args, expected) in [
            (json!({"app":""}), "app_open requires"),
            (
                json!({"app":"Blender","new_instance":"false"}),
                "new_instance must be a boolean",
            ),
            (
                json!({"app":"Blender","action":"click"}),
                "app_open does not accept action",
            ),
            (json!({"app":"Blender","x":1}), "app_open does not accept x"),
        ] {
            args["workspace"] = json!(workspace);
            let rejected = fixture
                .tool(
                    "app_open",
                    with_context(args, "Launcher restored; reject invalid input"),
                )
                .await;
            assert_eq!(rejected["isError"], true, "{rejected}");
            assert!(
                rejected["structuredContent"]["result"]["error"]
                    .as_str()
                    .unwrap()
                    .contains(expected),
                "{rejected}"
            );
        }
    }

    #[test]
    fn advertised_tools_include_structured_output_schema() {
        let tools = tool_definitions();
        let bash = tools.iter().find(|tool| tool["name"] == "bash").unwrap();
        let schema = &bash["outputSchema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["result", "time_cost_ms"]));
        assert_eq!(schema["properties"]["result"]["type"], "object");
        assert_eq!(schema["properties"]["time_cost_ms"]["type"], "integer");
        assert!(tools.iter().all(|tool| tool["outputSchema"].is_object()));
    }

    #[test]
    fn summary_and_context_validation_match_new_contract() {
        for invalid in [
            Value::Null,
            json!([]),
            json!({}),
            json!({"summary":null}),
            json!({"summary":""}),
            json!({"summary":" \t\n"}),
            json!({"summary":"x".repeat(MAX_SUMMARY_CHARS + 1)}),
            json!({"summary":"雪".repeat(MAX_SUMMARY_CHARS + 1)}),
        ] {
            assert!(call_summary(&invalid).is_err(), "accepted {invalid}");
        }
        for valid in [
            "x".to_owned(),
            "Mapped schema; run unit check".to_owned(),
            "雪".repeat(MAX_SUMMARY_CHARS),
        ] {
            assert_eq!(call_summary(&json!({"summary":valid})).unwrap(), valid);
        }
        let valid = with_context(json!({}), "Mapped schema; validate metadata");
        let metadata = call_metadata(&valid).unwrap();
        assert_eq!(metadata["main_task"], "Verify MCP metadata");
        assert_eq!(metadata["current_task"], "Run focused unit check");
        assert_eq!(metadata["progress"], 60);
        assert_eq!(metadata["quality"], 95);
        for field in [
            "agent",
            "model",
            "main_task",
            "current_task",
            "current_timestamp",
        ] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                call_metadata(&missing)
                    .unwrap_err()
                    .to_string()
                    .contains(field)
            );
            let mut blank = valid.clone();
            blank[field] = json!("  \n\t");
            assert!(
                call_metadata(&blank)
                    .unwrap_err()
                    .to_string()
                    .contains(field)
            );
        }
        for (field, max_chars) in [
            ("agent", MAX_AGENT_CHARS),
            ("model", MAX_MODEL_CHARS),
            ("main_task", MAX_MAIN_TASK_CHARS),
            ("current_task", MAX_CURRENT_TASK_CHARS),
            ("current_timestamp", MAX_CURRENT_TIMESTAMP_CHARS),
        ] {
            let mut too_long = valid.clone();
            too_long[field] = json!("雪".repeat(max_chars + 1));
            assert!(
                call_metadata(&too_long)
                    .unwrap_err()
                    .to_string()
                    .contains(field)
            );
        }
        for field in ["progress", "quality"] {
            for bad in [json!(-1), json!(101), json!(12.5), json!("50"), Value::Null] {
                let mut invalid = valid.clone();
                invalid[field] = bad;
                assert!(
                    call_metadata(&invalid)
                        .unwrap_err()
                        .to_string()
                        .contains(field)
                );
            }
        }
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
    async fn handshake_and_workspace_bootstrap_point_to_bash_python_guidance() {
        let fixture = TestApp::new();
        let init = fixture.request("initialize", json!({})).await;
        let instructions = init["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains(crate::resources::INSTRUCTION_URI));
        assert!(instructions.contains("bash or python"));
        assert!(!instructions.contains("read_file first"));
        let opened = fixture
            .tool(
                "workspace_open",
                with_context(json!({"path":fixture.root}), "Found folder; open workspace"),
            )
            .await;
        assert_eq!(opened["isError"], false);
        let bootstrap = opened["structuredContent"]["instructions"]
            .as_str()
            .unwrap();
        assert!(bootstrap.contains("Agents.md"));
        assert!(bootstrap.contains("bash or python"));
        assert!(opened["structuredContent"]["result"]["id"].is_string());
    }

    #[tokio::test]
    async fn every_tool_rejects_invalid_summary_before_dispatch() {
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
            }
        }
        assert!(fixture.app.disk.lock().unwrap().workspaces.is_empty());
    }

    #[tokio::test]
    async fn bash_replaces_public_file_write_and_logs_new_metadata() {
        let fixture = TestApp::new();
        let opened = fixture
            .tool(
                "workspace_open",
                with_context(json!({"path":fixture.root}), "Found folder; open workspace"),
            )
            .await;
        let workspace = opened["structuredContent"]["result"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let removed = fixture
            .tool(
                "write_file",
                with_context(
                    json!({"workspace":workspace,"path":"sentinel.txt","text":"bad"}),
                    "Opened workspace; reject write_file",
                ),
            )
            .await;
        assert_eq!(removed["isError"], true);
        assert!(!fixture.root.join("sentinel.txt").exists());
        let result = fixture
            .tool(
                "bash",
                with_context(
                    json!({"workspace":workspace,"command":"printf verified > sentinel.txt"}),
                    "Opened workspace; write via bash",
                ),
            )
            .await;
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("sentinel.txt")).unwrap(),
            "verified"
        );
        for field in [
            "summary",
            "agent",
            "model",
            "main_task",
            "current_task",
            "progress",
            "quality",
            "current_timestamp",
        ] {
            assert!(result["structuredContent"].get(field).is_none());
        }
        let logs = fixture.app.disk.lock().unwrap().logs.clone();
        let call = logs.last().unwrap();
        assert_eq!(call["details"]["source"], "MCP");
        assert_eq!(call["details"]["main_task"], "Verify MCP metadata");
        assert_eq!(call["details"]["current_task"], "Run focused unit check");
        assert_eq!(call["details"]["progress"], 60);
        assert_eq!(call["details"]["quality"], 95);
        assert_eq!(
            call["details"]["summary"],
            "Opened workspace; write via bash"
        );
        assert!(call["details"]["output"].is_object());
    }

    #[tokio::test]
    async fn wait_and_resource_bridges_keep_new_metadata_contract() {
        let fixture = TestApp::new();
        let waited = fixture
            .tool(
                "wait_n",
                with_context(json!({"seconds":0.01}), "Metadata ready; test wait tool"),
            )
            .await;
        assert_eq!(waited["isError"], false);
        assert!(
            waited["structuredContent"]["time_cost_ms"]
                .as_u64()
                .unwrap()
                >= 5
        );
        let protocol = fixture.request("resources/list", json!({})).await;
        let bridge = fixture
            .tool(
                "list_resources",
                with_context(json!({}), "Metadata ready; list resources"),
            )
            .await;
        assert_eq!(bridge["structuredContent"]["result"], protocol["result"]);
        for name in [
            "list_resources",
            "read_resource",
            "workspace_list",
            "get_screenshot",
            "list_windows",
            "wait_n",
        ] {
            assert_eq!(tool_annotations(name)["readOnlyHint"], true, "{name}");
            assert_eq!(tool_annotations(name)["destructiveHint"], false, "{name}");
        }
        assert_eq!(tool_annotations("virtual_pointer")["readOnlyHint"], false);
    }
}
