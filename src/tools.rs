use crate::{Result, context, err, state::App};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

pub fn definitions() -> Vec<Value> {
    let browser_size = crate::computer::browser_size_schema();
    vec![
        json!({"name":"shell","description":"Run bash in a visible persistent PTY. Returns terminal_id and output; running commands continue in the backend. Use terminal_read to poll, terminal_write for input, terminal_stop to terminate.","parameters":{"type":"object","properties":{"command":{"type":"string"},"wait_ms":{"type":"integer"}},"required":["command"],"additionalProperties":false}}),
        json!({"name":"bash","description":"Run Bash code in a workspace PTY. Returns terminal_id, output and exit status; use terminal_read/write/stop for running programs.","parameters":{"type":"object","properties":{"command":{"type":"string"},"wait_ms":{"type":"integer","minimum":0,"maximum":10000}},"required":["command"],"additionalProperties":false}}),
        json!({"name":"python","description":"Run Python 3 code in the workspace using an unbuffered PTY. Each call starts a new interpreter. Returns terminal_id, output and exit status; use terminal_read/write/stop for running programs.","parameters":{"type":"object","properties":{"code":{"type":"string"},"wait_ms":{"type":"integer","minimum":0,"maximum":10000}},"required":["code"],"additionalProperties":false}}),
        json!({"name":"write_image","description":"Receive a base64 image content block, save it in the workspace, and reply with the same image. Supports PNG, JPEG, GIF and WebP up to 20 MiB.","parameters":{"type":"object","properties":{"path":{"type":"string"},"image":{"type":"object","properties":{"type":{"type":"string","enum":["image"]},"mimeType":{"type":"string","enum":["image/png","image/jpeg","image/gif","image/webp"]},"data":{"type":"string"}},"required":["type","mimeType","data"],"additionalProperties":false}},"required":["path","image"],"additionalProperties":false}}),
        json!({"name":"terminal_read","description":"Wait briefly for output or completion of a virtual terminal. If a server is running, verify it via HTTP instead of polling for exit. Stop unbounded searches and use scoped commands.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"},"wait_ms":{"type":"integer"}},"required":["terminal_id"],"additionalProperties":false}}),
        json!({"name":"terminal_write","description":"Send text or control characters to a virtual terminal. Include a newline to submit a command.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"},"text":{"type":"string"}},"required":["terminal_id","text"],"additionalProperties":false}}),
        json!({"name":"terminal_stop","description":"Stop a running terminal.","parameters":{"type":"object","properties":{"terminal_id":{"type":"string"}},"required":["terminal_id"],"additionalProperties":false}}),
        json!({"name":"read_file","description":"Read a workspace UTF-8 file or image. Supports byte offset and limit for text.","parameters":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"],"additionalProperties":false}}),
        json!({"name":"write_file","description":"Create or replace a workspace text file. Read existing files before replacing them.","parameters":{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"}},"required":["path","text"],"additionalProperties":false}}),
        json!({"name":"list_files","description":"List project files respecting gitignore, with context token estimates.","parameters":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"browser_open","description":"Open a new controlled background Chrome window, default 1000 by 600 logical points (fitted to the primary display). Maximum width/height follow the current display, not hard-coded constants. Use project output/ for important artifacts. Opens with an isolated profile and independent pointer/button state. Does not enable debugging on the user's normal profile. Returns window_id, pid and an automatic page screenshot padded to window coordinates (browser toolbar is marked uncaptured); use computer with both IDs for subsequent input. Requires computer control enabled. Browser input is delivered through Chrome's local input protocol, never the system pointer.","parameters":{"type":"object","properties":{"url":{"type":"string"},"width":browser_size["width"],"height":browser_size["height"],"capture_path":{"type":"string"}},"required":["url"],"additionalProperties":false}}),
        json!({"name":"computer","description":"Control the desktop: windows (macOS), screenshot, move, click, drag, type, key, or scroll. For Chrome, first use browser_open (or action browser_open with url). Unmanaged Chrome windows are read-only and reject input. Never substitute the native or system-pointer path when the isolated channel is missing. Controlled browser_open Chrome uses its loopback input protocol and raw page-surface capture. Its uncaptured browser toolbar is a labeled neutral band so window-relative coordinates remain exact, including in Stage Manager; native-window control remains process-directed and never uses the system pointer. Browser chrome/title bars and multi-tab managed windows are not accepted. On macOS ONLY background control is supported: call windows, select window_id and pid, then pass both on every action. Targeted coordinates are relative to the whole window top-left; screenshot the same window first. Background events are posted once to the exact process/window and never move the physical pointer or activate the foreground app. A temporary recipient-local responder may report document focus without bringing the app forward; a light blue virtual pointer turns dark blue while left-pressed or dragging. After 10 seconds idle its fill is transparent with an outline still visible; at 30 seconds it disappears. New input restores it; screenshots do not reset the timer. Every successful input action waits two seconds then automatically captures the target window (or desktop for desktop mode), saves it to output, and returns the image before the next model request. Take an explicit screenshot only for the initial view or to recover from screenshot_error; use automatic observations thereafter. The app must accept background events and the window must be on the current desktop, not minimized. key/type target the app keyboard responder; click the intended input first. scroll needs x,y in background mode. drag may use path:[[x,y],...] for a continuous stroke. On macOS every input requires window_id and pid. Desktop mode is rejected, including explicit requests; there is no global-input fallback. Missing or malformed targets fail without input. Linux X11 retains desktop control. Take a screenshot first when coordinates matter. Screenshot results include the exact screenshot pixel width/height; pass those values as screen_width and screen_height when acting on that image. Lessagent maps screenshot pixels to macOS logical points on Retina displays. A drag normally uses integer x,y,to_x,to_y; a horizontal or vertical drag may omit the unchanged destination axis and Lessagent keeps it at the starting coordinate. from/to or start_x,start_y,end_x,end_y aliases are also accepted and normalized before input is posted. duration is optional seconds (duration_ms is accepted too). key accepts modifier+key (cmd+a on macOS). Scroll at x,y using distance: positive scrolls up (+10 = 10 lines up), negative scrolls down. distance is limited to -100..100. Legacy delta has the opposite sign; supply only one. Desktop scroll may omit both coordinates to use the current pointer. Requires computer control enabled in Settings.","parameters":{"type":"object","properties":{"action":{"type":"string","enum":["windows","screenshot","browser_open","move","click","drag","type","key","scroll"]},"url":{"type":"string","description":"HTTP(S) URL for browser_open only."},"width":browser_size["width"],"height":browser_size["height"],"window_id":{"type":"integer","minimum":1},"pid":{"type":"integer","minimum":1},"mode":{"type":"string","enum":if cfg!(target_os = "macos") { json!(["background"]) } else { json!(["desktop"]) },"description":"macOS supports background only, requiring window_id and pid. Desktop is Linux-only and is rejected on macOS."},"capture_path":{"type":"string","description":"Optional workspace-relative path for the screenshot PNG."},"show_pointer":{"type":"boolean"},"path":{"type":"array","minItems":2,"maxItems":512,"items":{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2}},"x":{"type":"integer"},"y":{"type":"integer"},"to_x":{"type":"integer"},"to_y":{"type":"integer"},"from":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"to":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"start":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"end":{"type":"array","items":{"type":"integer"},"minItems":2,"maxItems":2},"start_x":{"type":"integer"},"start_y":{"type":"integer"},"end_x":{"type":"integer"},"end_y":{"type":"integer"},"target_x":{"type":"integer"},"target_y":{"type":"integer"},"duration":{"type":"number","minimum":0.05,"maximum":5},"duration_ms":{"type":"number","minimum":50,"maximum":5000},"screen_width":{"type":"integer"},"screen_height":{"type":"integer"},"button":{"type":"string","enum":["left","right"]},"text":{"type":"string"},"key":{"type":"string"},"distance":{"type":"integer","minimum":-100,"maximum":100,"description":"Lines: positive up, negative down; exclusive with legacy delta."},"delta":{"type":"integer","description":"Legacy lines: positive down, negative up."}},"required":["action"],"additionalProperties":false}}),
    ]
}
pub async fn execute(app: Arc<App>, workspace: &str, name: &str, args: &Value) -> Result<Value> {
    let w = app.workspace(workspace)?;
    app.log("tool", &format!("{name} · {}", w.name));
    match name {
        "shell" | "bash" | "python" => {
            let source = string(args, if name == "python" { "code" } else { "command" })?;
            if source.len() > 128 * 1024 {
                return Err(err("Command too long"));
            }
            let python_command;
            let command = if name == "python" {
                python_command = format!("python3 -u -c '{}'", source.replace('\'', "'\"'\"'"));
                &python_command
            } else {
                source
            };
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
                let mut result = json!({"path":args["path"]});
                crate::image_content::attach(&mut result, &bytes, context::mime(&p))?;
                return Ok(result);
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
        "write_image" => {
            use base64::Engine;
            let image = &args["image"];
            if image["type"] != "image" {
                return Err(err("Expected image content block"));
            }
            let mime = string(image, "mimeType")?;
            let data = string(image, "data")?;
            if data.len() > 28 * 1024 * 1024 {
                return Err(err("Image exceeds 20 MiB limit"));
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
            if bytes.len() > 20 * 1024 * 1024 {
                return Err(err("Image exceeds 20 MiB limit"));
            }
            let detected = match imagesize::image_type(&bytes)? {
                imagesize::ImageType::Png => "image/png",
                imagesize::ImageType::Jpeg => "image/jpeg",
                imagesize::ImageType::Gif => "image/gif",
                imagesize::ImageType::Webp => "image/webp",
                _ => return Err(err("Unsupported image format")),
            };
            let p = context::resolve(&w.path, string(args, "path")?, true)?;
            if mime != detected || context::mime(&p) != detected {
                return Err(err("Image bytes, mimeType and file extension must agree"));
            }
            let mut result = json!({"path":args["path"]});
            crate::image_content::attach(&mut result, &bytes, mime)?;
            crate::state::atomic_write(&p, &bytes)?;
            Ok(result)
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
        "computer" | "browser_open" => {
            if !app.disk.lock().unwrap().settings.computer_enabled {
                return Err(err(
                    "Enable computer control in Settings before using desktop tools",
                ));
            }
            // Agent calls choose the iteration artifact directory; direct calls
            // save automatic observations in the workspace output tree.
            let capture_relative = args["capture_path"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("output/computer/{}.png", crate::id()));
            let p = context::resolve(&w.path, &capture_relative, true)?;
            let mut args = args.clone();
            if name == "browser_open" {
                args["action"] = json!("browser_open");
            }
            // This private path is supplied by the backend, never trusted from model arguments.
            args["_browser_root"] = json!(app.dir.join("browsers"));
            let result = tokio::task::spawn_blocking(move || {
                // Keep input and its observation together, including concurrent MCP calls.
                static CONTROL: std::sync::Mutex<()> = std::sync::Mutex::new(());
                let _guard = CONTROL
                    .lock()
                    .map_err(|_| err("Computer control lock poisoned"))?;
                let mut r = crate::computer::action(&args, &p)?;
                let control = matches!(
                    args["action"].as_str(),
                    Some("browser_open" | "move" | "click" | "drag" | "type" | "key" | "scroll")
                );
                if control {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let mut observation = json!({"action":"screenshot"});
                    for key in ["window_id", "pid", "mode", "show_pointer", "_browser_root"] {
                        if let Some(value) = args.get(key).or_else(|| r.get(key)) {
                            observation[key] = value.clone();
                        }
                    }
                    match crate::computer::action(&observation, &p) {
                        Ok(shot) => {
                            if let Some(fields) = shot.as_object() {
                                for (key, value) in fields {
                                    r[key] = value.clone();
                                }
                            }
                            r["automatic_screenshot"] = json!(true);
                            r["observation_delay_ms"] = json!(2000);
                        }
                        Err(error) => {
                            // Input already happened. Preserve that fact so callers do
                            // not repeat an action merely because capture failed.
                            r["screenshot_error"] = json!(error.to_string());
                            r["automatic_screenshot"] = json!(false);
                        }
                    }
                }
                if args["action"] == "screenshot" || r["automatic_screenshot"] == true {
                    let encoded = std::fs::read(&p).map_err(crate::Error::from)
                        .and_then(|bytes| crate::image_content::attach(&mut r, &bytes, "image/png"));
                    if let Err(error) = encoded {
                        if !control { return Err(error); }
                        // Input already occurred: do not turn an observation failure
                        // into a retryable input failure or replay the action.
                        r["screenshot_error"] = json!(error.to_string());
                        r["automatic_screenshot"] = json!(false);
                    }
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
