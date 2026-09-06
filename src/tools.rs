use crate::{Result, context, err, state::App};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

pub fn definitions() -> Vec<Value> {
    vec![
        json!({"name":"shell","description":"Run bash in a visible persistent PTY. Returns terminal_id and output; running commands continue in the backend. Use terminal_read to poll, terminal_write for input, terminal_stop to terminate.","parameters":{"type":"object","properties":{"command":{"type":"string"},"wait_ms":{"type":"integer"}},"required":["command"],"additionalProperties":false}}),
        json!({"name":"terminal_read","description":"Read output and exit status of a virtual terminal.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"}},"required":["terminal_id"],"additionalProperties":false}}),
        json!({"name":"terminal_write","description":"Send text or control characters to a virtual terminal. Include a newline to submit a command.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"},"text":{"type":"string"}},"required":["terminal_id","text"],"additionalProperties":false}}),
        json!({"name":"terminal_stop","description":"Stop a running terminal.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"}},"required":["terminal_id"],"additionalProperties":false}}),
        json!({"name":"read_file","description":"Read a workspace UTF-8 file or image. Supports byte offset and limit for text.","parameters":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"],"additionalProperties":false}}),
        json!({"name":"write_file","description":"Create or replace a workspace text file. Read existing files before replacing them.","parameters":{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"}},"required":["path","text"],"additionalProperties":false}}),
        json!({"name":"list_files","description":"List project files respecting gitignore, with context token estimates.","parameters":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"computer","description":"Control desktop: screenshot, move, click, drag, type, key, scroll. Coordinates are screen coordinates; key accepts modifier+key (cmd+a on macOS). Positive scroll delta scrolls down. Requires computer control enabled in Settings.","parameters":{"type":"object","properties":{"action":{"type":"string","enum":["screenshot","move","click","drag","type","key","scroll"]},"x":{"type":"integer"},"y":{"type":"integer"},"to_x":{"type":"integer"},"to_y":{"type":"integer"},"button":{"type":"string","enum":["left","right"]},"text":{"type":"string"},"key":{"type":"string"},"delta":{"type":"integer"}},"required":["action"],"additionalProperties":false}}),
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
            let ignores = app.disk.lock().unwrap().settings.context_ignores.clone();
            let inventory = tokio::task::spawn_blocking(move || {
                context::scan_with_ignores(&w.path, false, &ignores)
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
            let p = context::resolve(
                &w.path,
                &format!("agent/captures/{}.png", crate::id()),
                true,
            )?;
            let args = args.clone();
            let result=tokio::task::spawn_blocking(move||{let mut r=crate::computer::action(&args,&p)?;if args["action"]=="screenshot"{use base64::Engine;r["image"]=json!({"mime":"image/png","data":base64::engine::general_purpose::STANDARD.encode(std::fs::read(&p)?)});}Ok::<_,crate::Error>(r)}).await??;
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
