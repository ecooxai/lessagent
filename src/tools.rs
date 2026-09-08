use crate::{Result, context, err, state::App};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

pub fn definitions() -> Vec<Value> {
    vec![
        json!({"name":"shell","description":"Run bash in a visible persistent PTY. Returns terminal_id and output; running commands continue in the backend. Use terminal_read to poll, terminal_write for input, terminal_stop to terminate.","parameters":{"type":"object","properties":{"command":{"type":"string"},"wait_ms":{"type":"integer"}},"required":["command"],"additionalProperties":false}}),
        json!({"name":"terminal_read","description":"Wait briefly for output or completion of a virtual terminal. If a server is running, verify it via HTTP instead of polling for exit. Stop unbounded searches and use scoped commands.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"},"wait_ms":{"type":"integer"}},"required":["terminal_id"],"additionalProperties":false}}),
        json!({"name":"terminal_write","description":"Send text or control characters to a virtual terminal. Include a newline to submit a command.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"},"text":{"type":"string"}},"required":["terminal_id","text"],"additionalProperties":false}}),
        json!({"name":"terminal_stop","description":"Stop a running terminal.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"}},"required":["terminal_id"],"additionalProperties":false}}),
        json!({"name":"read_file","description":"Read a workspace UTF-8 file or image. Supports byte offset and limit for text.","parameters":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"],"additionalProperties":false}}),
        json!({"name":"write_file","description":"Create or replace a workspace text file. Read existing files before replacing them.","parameters":{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"}},"required":["path","text"],"additionalProperties":false}}),
        json!({"name":"list_files","description":"List project files respecting gitignore, with context token estimates.","parameters":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"computer","description":"Control the desktop: screenshot, move, click, drag, type, key, or scroll. Take a screenshot first when coordinates matter. Screenshot results include the exact screenshot pixel width/height; pass those values as screen_width and screen_height when acting on that image. Lessagent maps screenshot pixels to macOS logical points on Retina displays. A drag normally uses integer x,y,to_x,to_y; a horizontal or vertical drag may omit the unchanged destination axis and Lessagent keeps it at the starting coordinate. from/to or start_x,start_y,end_x,end_y aliases are also accepted and normalized before input is posted. duration is optional seconds (duration_ms is accepted too). key accepts modifier+key (cmd+a on macOS). Positive scroll delta scrolls down. Requires computer control enabled in Settings.","parameters":{"type":"object","properties":{"action":{"type":"string","enum":["screenshot","move","click","drag","type","key","scroll"]},"x":{"type":"integer"},"y":{"type":"integer"},"to_x":{"type":"integer"},"to_y":{"type":"integer"},"from":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"to":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"start":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"end":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"start_x":{"type":"integer"},"start_y":{"type":"integer"},"end_x":{"type":"integer"},"end_y":{"type":"integer"},"target_x":{"type":"integer"},"target_y":{"type":"integer"},"duration":{"type":"number","minimum":0.05,"maximum":5},"duration_ms":{"type":"number","minimum":50,"maximum":5000},"screen_width":{"type":"integer"},"screen_height":{"type":"integer"},"button":{"type":"string","enum":["left","right"]},"text":{"type":"string"},"key":{"type":"string"},"delta":{"type":"integer"}},"required":["action"],"additionalProperties":false}}),
    ]
}
pub async fn execute(app: Arc<App>, workspace: &str, name: &str, args: &Value) -> Result<Value> {
    let w = app.workspace(workspace)?;
    app.log("tool", &format!("{name} · {}", w.name));
    match name {
        "shell" => {
            let command = string(args, "command")?;
            if command.len() > 128 * 1024 {
                return Err(err("Command too long"));
            }
            let t = app
                .terminals
                .spawn_owned(workspace, &w.path, Some(command), true)?;
            let wait = args["wait_ms"].as_u64().unwrap_or(1000).min(10_000);
            let end = tokio::time::Instant::now() + Duration::from_millis(wait);
            while !t.exited.load(Ordering::SeqCst) && tokio::time::Instant::now() < end {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(
                json!({"terminal_id":t.id,"output":crate::tail(&t.output(),24_000),"exited":t.exited.load(Ordering::SeqCst),"exit_code":t.snapshot()["exit_code"]}),
            )
        }
        "terminal_read" | "terminal_write" | "terminal_stop" => {
            let t = app.terminals.get(string(args, "terminal_id")?)?;
            if t.workspace != workspace {
                return Err(err("Terminal belongs to another workspace"));
            }
            if name == "terminal_read" {
                let initial = t.output();
                let end = tokio::time::Instant::now()
                    + Duration::from_millis(args["wait_ms"].as_u64().unwrap_or(2000).min(10_000));
                while !t.exited.load(Ordering::SeqCst)
                    && t.output() == initial
                    && tokio::time::Instant::now() < end
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            if name == "terminal_write" {
                t.write(string(args, "text")?)?;
            }
            if name == "terminal_stop" {
                t.kill()?;
            }
            let mut result = t.snapshot();
            result["output"] = json!(crate::tail(&t.output(), 24_000));
            Ok(result)
        }
        "read_file" => {
            let p = context::resolve(&w.path, string(args, "path")?, false)?;
            if p.metadata()?.len() > 64 * 1024 * 1024 {
                return Err(err("File exceeds 64 MiB read limit"));
            }
            let bytes = std::fs::read(&p)?;
            if matches!(
                context::mime(&p),
                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
            ) {
                use base64::Engine;
                return Ok(
                    json!({"image":{"mime":context::mime(&p),"data":base64::engine::general_purpose::STANDARD.encode(bytes)},"path":args["path"]}),
                );
            }
            let text = String::from_utf8(bytes)?;
            let mut start = (args["offset"].as_u64().unwrap_or(0) as usize).min(text.len());
            while !text.is_char_boundary(start) {
                start += 1;
            }
            let limit = (args["limit"].as_u64().unwrap_or(24_000) as usize).clamp(1, 100_000);
            Ok(
                json!({"text":crate::clip(&text[start..],limit),"offset":start,"total_bytes":text.len()}),
            )
        }
        "write_file" => {
            let p = context::resolve(&w.path, string(args, "path")?, true)?;
            let text = string(args, "text")?;
            crate::state::atomic_write(&p, text.as_bytes())?;
            Ok(json!({"written":args["path"],"bytes":text.len()}))
        }
        "list_files" => {
            let settings = app.disk.lock().unwrap().settings.clone();
            let ignores = settings.context_ignores;
            let model = settings.model;
            let inventory = tokio::task::spawn_blocking(move || {
                context::scan_for_model(&w.path, false, &ignores, &model)
            })
            .await??;
            Ok(serde_json::to_value(inventory.inventory)?)
        }
        "computer" => {
            if !app.disk.lock().unwrap().settings.computer_enabled {
                return Err(err(
                    "Enable computer control in Settings before using desktop tools",
                ));
            }
            // The agent loop injects an artifact path for screenshots. Direct
            // CLI/MCP calls retain the legacy capture location as a fallback.
            let capture_relative = args["capture_path"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("agent/captures/{}.png", crate::id()));
            let p = context::resolve(&w.path, &capture_relative, true)?;
            let args = args.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut r = crate::computer::action(&args, &p)?;
                if args["action"] == "screenshot" {
                    use base64::Engine;
                    r["image"] = json!({
                        "mime":"image/png",
                        "data":base64::engine::general_purpose::STANDARD.encode(std::fs::read(&p)?),
                    });
                }
                Ok::<_, crate::Error>(r)
            })
            .await??;
            Ok(result)
        }
        _ => Err(err(format!("Unknown tool: {name}"))),
    }
}
pub fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| err(format!("Missing string {key}")))
}
