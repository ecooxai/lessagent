use crate::{Result, err, state::App, tools::string};
use axum::{
    Json, Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use serde_json::{Value, json};
use std::sync::Arc;
struct ApiError(crate::Error);
impl<E: Into<crate::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}
type Api = std::result::Result<Json<Value>, ApiError>;
pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/api/state", get(state))
        .route("/api/login", post(login))
        .route("/api/admin/shutdown", post(stop_server))
        .route("/api/admin/password", post(set_password))
        .route("/api/action/{action}", post(action))
        .route("/api/models/{provider}", get(models))
        .route("/api/inventory/{workspace}", get(inventory))
        .route("/api/media/{workspace}", post(media))
        .route("/api/audio/{action}", post(audio))
        .route("/mcp", get(mcp_info).post(mcp))
        .layer(middleware::from_fn_with_state(app.clone(), auth));
    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route(
            "/health",
            get(|| async { Json(json!({"service":"lessagent","ok":true,"management_api":1,"build":crate::build_info(),"pid":std::process::id()})) }),
        )
        .merge(api)
        .layer(DefaultBodyLimit::max(40 * 1024 * 1024))
        .layer(middleware::from_fn(headers))
        .with_state(app)
}
async fn headers(req: Request, next: Next) -> Response {
    let mut r = next.run(req).await;
    for (k, v) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self' https://api.openai.com; frame-src https: http:; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        r.headers_mut().insert(
            axum::http::HeaderName::from_static(k),
            axum::http::HeaderValue::from_static(v),
        );
    }
    r
}
async fn auth(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    // GET /mcp is a public, read-only health/introspection surface used by the
    // standalone service checker, including through HTTPS tunnels.
    if req.uri().path() == "/mcp" && req.method() == Method::GET {
        return next.run(req).await;
    }
    let h = req.headers();
    let host = h
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let local = host == "localhost"
        || host == "127.0.0.1"
        || host.starts_with("localhost:")
        || host.starts_with("127.0.0.1:");
    let local = local
        && req
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .is_some_and(|peer| peer.0.ip().is_loopback());
    let origin_ok = h
        .get(header::ORIGIN)
        .map(|o| {
            o.to_str().ok().is_some_and(|origin| {
                origin == format!("http://{host}") || origin == format!("https://{host}")
            })
        })
        .unwrap_or(true);
    let cross_site = h.get("sec-fetch-site").is_some_and(|v| v == "cross-site");
    if host.is_empty() || !origin_ok || cross_site {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Same-origin requests only"})),
        )
            .into_response();
    }
    let authorization = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    let authenticated = authorization == Some(format!("Bearer {}", app.token).as_str());
    let path = req.uri().path();
    // MCP is deliberately public, including remote clients and installations
    // with a browser password. Retain the origin checks above for browser calls.
    if path == "/mcp" {
        return next.run(req).await;
    }
    let token_required = path.starts_with("/api/admin/");
    let password_required = app.password.lock().unwrap().is_some();
    if path != "/api/login"
        && !authenticated
        && (token_required || password_required || !local || authorization.is_some())
    {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"Sign in with your password or use a valid access token","password_required":password_required}))).into_response();
    }
    next.run(req).await
}
async fn login(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Response {
    let hash = app.password.lock().unwrap().clone();
    let Some(hash) = hash else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"No password is set. Set one with lessagent --passwd PASSWORD."})),
        )
            .into_response();
    };
    let password = body["password"].as_str().unwrap_or("").to_owned();
    let valid = tokio::task::spawn_blocking(move || {
        use argon2::{Argon2, PasswordHash, PasswordVerifier};
        PasswordHash::new(&hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false);
    if valid {
        Json(json!({"token":app.token})).into_response()
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Incorrect password"})),
        )
            .into_response()
    }
}
async fn stop_server(State(app): State<Arc<App>>) -> Json<Value> {
    app.shutdown.notify_one();
    Json(json!({"ok":true}))
}
async fn set_password(State(app): State<Arc<App>>, Json(body): Json<Value>) -> Api {
    let password = string(&body, "password")?.to_owned();
    tokio::task::spawn_blocking(move || app.set_password(&password))
        .await
        .map_err(|e| err(e.to_string()))??;
    Ok(Json(json!({"ok":true})))
}
async fn state(
    State(app): State<Arc<App>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let mut v = {
        let disk = app.disk.lock().unwrap();
        if query.get("terminal").is_some_and(|v| v == "true") {
            let workspaces: Vec<_> = disk.workspaces.iter().map(|w| json!({"id":w.id,"name":w.name,"path":w.path,"mode":w.mode,"closed":w.closed,"ui":w.ui})).collect();
            let latest_log = disk.logs.last().map(
                |entry| json!({"at":entry["at"],"kind":entry["kind"],"message":entry["message"]}),
            );
            json!({"partial":true,"workspaces":workspaces,"settings":disk.settings,"ui":disk.ui,"latest_log":latest_log})
        } else {
            serde_json::to_value(&*disk).unwrap()
        }
    };
    v["debug_build"] = json!(cfg!(debug_assertions));
    v["terminals"] = json!(if query.get("summary").is_some_and(|v| v == "true") {
        app.terminals.summaries()
    } else {
        app.terminals.list()
    });
    v["keys"] = json!({"openai":app.key("openai").is_ok(),"claude":app.key("claude").is_ok(),"gemini":app.key("gemini").is_ok()});
    v["computer"] = crate::computer::capabilities();
    v["context_ignore_defaults"] = json!(crate::context::default_ignores());
    Json(v)
}
async fn models(State(app): State<Arc<App>>, Path(provider): Path<String>) -> Api {
    Ok(Json(crate::provider::models(app, &provider).await?))
}
async fn inventory(State(app): State<Arc<App>>, Path(id): Path<String>) -> Api {
    let w = app.workspace(&id)?;
    let settings = app.disk.lock().unwrap().settings.clone();
    let ignores = settings.context_ignores;
    let model = if settings.model.is_empty() && settings.provider == "codex" {
        crate::provider::models(app.clone(), "codex").await?[0]["id"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned()
    } else {
        settings.model
    };
    let b = tokio::task::spawn_blocking(move || {
        crate::context::scan_for_model(&w.path, false, &ignores, &model)
    })
    .await??;
    Ok(Json(json!(b.inventory)))
}

/// Keep screenshots returned by the normal HTTP tool endpoint lightweight.
///
/// `tools::execute` also serves the agent loop and the MCP bridge, both of
/// which need the encoded image for model feedback.  The browser-facing HTTP
/// endpoint only needs the artifact path (and the dimensions already returned
/// by the shared computer backend), so remove the binary payload at this boundary.
fn strip_api_screenshot_image(name: &str, args: &Value, result: &mut Value) {
    let screenshot_result = name == "get_screenshot"
        || args["action"] == "screenshot"
        || result["automatic_screenshot"] == true;
    if crate::tools::is_computer_tool(name)
        && screenshot_result
        && let Some(object) = result.as_object_mut()
    {
        object.remove("image");
    }
}

async fn action(
    State(app): State<Arc<App>>,
    Path(action): Path<String>,
    Json(a): Json<Value>,
) -> Api {
    let result = match action.as_str() {
        "service_check_open" => {
            let base_url = string(&a, "base_url")?;
            let parsed = reqwest::Url::parse(base_url)?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(err("Service checker base URL must use HTTP or HTTPS").into());
            }
            let executable = std::env::current_exe()?;
            let mut child = std::process::Command::new(executable)
                .arg("service-check")
                .arg("--data-dir")
                .arg(&app.dir)
                .arg("--base-url")
                .arg(base_url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            json!({"ok":true,"pid":pid})
        }
        "event_read" => {
            let id = string(&a, "id")?;
            if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                return Err(err("Invalid event id").into());
            }
            let path = app.dir.join("events").join(format!("{id}.json"));
            let bytes = tokio::task::spawn_blocking(move || std::fs::read(path)).await??;
            return Ok(Json(serde_json::from_slice(&bytes)?));
        }
        "browse" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            let path = a["path"].as_str().unwrap_or(&home);
            let path = if path == "~" {
                home.clone()
            } else if let Some(rest) = path.strip_prefix("~/") {
                format!("{home}/{rest}")
            } else {
                path.into()
            };
            let dir = std::path::Path::new(&path).canonicalize()?;
            if let Some(id) = a["workspace"].as_str() {
                let w = app.workspace(id)?;
                if !dir.starts_with(w.path.canonicalize()?) {
                    return Err(err("Folder is outside workspace").into());
                }
            }
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&dir)? {
                let Ok(entry) = entry else { continue };
                let Ok(meta) = entry.path().metadata() else {
                    continue;
                };
                let link = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
                let timestamp = |time: std::io::Result<std::time::SystemTime>| {
                    time.ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                };
                entries.push(json!({"modified_ms":timestamp(meta.modified()),"created_ms":timestamp(meta.created()),"name":entry.file_name().to_string_lossy(),"path":entry.path(),"directory":meta.is_dir(),"symlink":link,"bytes":if meta.is_file() { Some(meta.len()) } else { None } }));
            }
            json!({"path":dir,"home":home,"entries":entries})
        }
        // These high-frequency operations do not mutate persistent application state.
        "terminal_screen" => {
            let terminal = app.terminals.get(string(&a, "terminal_id")?)?;
            terminal
                .wait_for_change(a["revision"].as_u64(), a["wait_ms"].as_u64().unwrap_or(0))
                .await;
            return Ok(Json(terminal.screen_view(
                a["revision"].as_u64(),
                a["scrollback"].as_u64().unwrap_or(0) as usize,
                a["compact"].as_bool().unwrap_or(false),
            )));
        }
        "terminal_input" => {
            let terminal = app.terminals.get(string(&a, "terminal_id")?)?;
            if terminal.workspace != string(&a, "workspace")? {
                return Err(err("Terminal belongs to another workspace").into());
            }
            terminal.write(string(&a, "text")?)?;
            return Ok(Json(json!({"ok": true})));
        }
        "terminal_cd" => {
            let w = app.workspace(string(&a, "workspace")?)?;
            let cwd = crate::context::resolve(&w.path, string(&a, "path")?, false)?;
            if !cwd.is_dir() {
                return Err(err("Not a folder").into());
            }
            let existing = a["terminal_id"]
                .as_str()
                .and_then(|id| app.terminals.get(id).ok());
            if let Some(t) = existing.filter(|t| {
                t.workspace == w.id
                    && !t.snapshot()["running"].as_bool().unwrap_or(true)
                    && !t.exited.load(std::sync::atomic::Ordering::SeqCst)
            }) {
                let quoted = cwd.to_string_lossy().replace('\'', "'\\''");
                t.write(&format!("\x15cd -- '{quoted}'\r"))?;
                *t.cwd.lock().unwrap() = cwd;
                t.snapshot()
            } else {
                app.terminals.spawn(&w.id, &cwd, None)?.snapshot()
            }
        }
        "git_status" => {
            let w = app.workspace(string(&a, "workspace")?)?;
            crate::git::status(&w.path).await?
        }
        "git_file" => {
            let w = app.workspace(string(&a, "workspace")?)?;
            crate::git::file_diff(&w.path, string(&a, "path")?).await?
        }
        "git_branch" => {
            let w = app.workspace(string(&a, "workspace")?)?;
            crate::git::branch_info(&w.path, string(&a, "branch")?).await?
        }
        "git_switch" | "git_create" | "git_discard" => {
            let w = app.workspace(string(&a, "workspace")?)?;
            if app
                .disk
                .lock()
                .unwrap()
                .jobs
                .iter()
                .any(|j| j.workspace == w.id && j.status == "running")
            {
                return Err(err("Wait for the running task before changing Git state").into());
            }
            crate::git::action(
                &w.path,
                &action,
                a["branch"].as_str().unwrap_or(""),
                a["confirmed"].as_bool().unwrap_or(false),
            )
            .await?
        }
        "codex_usage" => crate::provider::codex_usage(&app).await?,
        "workspace_close" => {
            let id = string(&a, "workspace")?;
            let mut disk = app.disk.lock().unwrap();
            disk.workspaces
                .iter_mut()
                .find(|w| w.id == id)
                .ok_or_else(|| err("Workspace not found"))?
                .closed = true;
            if disk.ui["selected"] == id {
                disk.ui["selected"] = json!(null);
            }
            json!({"ok":true})
        }
        "workspace_open" => json!(app.open_workspace(std::path::Path::new(string(&a, "path")?))?),
        "workspace_mode" => {
            let id = string(&a, "workspace")?;
            let mode = string(&a, "mode")?;
            if !["normal", "light"].contains(&mode) {
                return Err(err("Mode must be normal or light").into());
            }
            let w = app.workspace(id)?;
            if mode == "light" {
                let settings = app.disk.lock().unwrap().settings.clone();
                let ignores = settings.context_ignores;
                let model = if settings.model.is_empty() && settings.provider == "codex" {
                    crate::provider::models(app.clone(), "codex").await?[0]["id"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_owned()
                } else {
                    settings.model
                };
                let b = tokio::task::spawn_blocking(move || {
                    crate::context::scan_for_model(&w.path, false, &ignores, &model)
                })
                .await??;
                if !b.inventory.light_allowed {
                    return Err(err(
                        "Light mode needs a complete project below 30k estimated tokens",
                    )
                    .into());
                }
            }
            let mut d = app.disk.lock().unwrap();
            if d.jobs
                .iter()
                .any(|j| j.workspace == id && j.status == "running")
            {
                return Err(err("Stop the running job before changing mode").into());
            }
            d.workspaces.iter_mut().find(|w| w.id == id).unwrap().mode = mode.into();
            json!({"ok":true})
        }
        "ui" => {
            let mut d = app.disk.lock().unwrap();
            if let Some(id) = a["workspace"].as_str() {
                d.workspaces
                    .iter_mut()
                    .find(|w| w.id == id)
                    .ok_or_else(|| err("Workspace not found"))?
                    .ui = a["ui"].clone();
            } else {
                d.ui = a["ui"].clone();
            }
            json!({"ok":true})
        }
        "draft" => {
            let mut d = app.disk.lock().unwrap();
            let w = d
                .workspaces
                .iter_mut()
                .find(|w| Some(w.id.as_str()) == a["workspace"].as_str())
                .ok_or_else(|| err("Workspace not found"))?;
            let session = a["session"].as_str().unwrap_or("chat");
            if session == "chat" {
                w.ui["draft"] = json!(string(&a, "text")?);
            } else {
                if !w.ui["drafts"].is_object() {
                    w.ui["drafts"] = json!({});
                }
                w.ui["drafts"][session] = json!(string(&a, "text")?);
            }
            json!({"ok":true})
        }
        "settings" => {
            let settings: crate::state::Settings = serde_json::from_value(a.clone())?;
            if !["codex", "openai", "gemini", "claude"].contains(&settings.provider.as_str())
                || !(1..=100).contains(&settings.max_steps)
            {
                return Err(err("Invalid provider or step limit (1–100)").into());
            }
            if settings.compact_model.trim().is_empty() {
                return Err(err("Choose a compaction model").into());
            }
            crate::provider::validate_thinking(&settings.thinking)?;
            if !settings.quality_threshold.is_finite()
                || !(0. ..=10.).contains(&settings.quality_threshold)
            {
                return Err(err(
                    "Quality threshold must be between 0 (inclusive) and 10 (inclusive)",
                )
                .into());
            }
            let mut rules = ignore::gitignore::GitignoreBuilder::new(".");
            for pattern in &settings.context_ignores {
                rules.add_line(None, pattern)?;
            }
            rules.build()?;
            app.disk.lock().unwrap().settings = settings;
            json!({"ok":true})
        }
        "key" => {
            let provider = string(&a, "provider")?;
            if !["openai", "gemini", "claude"].contains(&provider) {
                return Err(err("Unknown key provider").into());
            }
            let mut keys = app.secrets.lock().unwrap();
            keys.insert(provider.into(), string(&a, "key")?.into());
            crate::state::atomic_write(&app.dir.join("keys.json"), &serde_json::to_vec(&*keys)?)?;
            json!({"ok":true})
        }
        "run" => {
            json!({"job_id":crate::agent::start_in_session(app.clone(),string(&a,"workspace")?,string(&a,"prompt")?,a["thinking"].as_str(),a["session"].as_str().unwrap_or("chat"))?})
        }
        "stop" => {
            crate::agent::stop(&app, string(&a, "job_id")?)?;
            json!({"ok":true})
        }
        "terminal_new" => {
            let w = app.workspace(string(&a, "workspace")?)?;
            let cwd = crate::context::resolve(&w.path, a["path"].as_str().unwrap_or("."), false)?;
            app.terminals
                .spawn(&w.id, &cwd, a["command"].as_str())?
                .snapshot()
        }
        "terminal_resize" => {
            let terminal = app.terminals.get(string(&a, "terminal_id")?)?;
            terminal.resize(
                a["rows"].as_u64().unwrap_or(24).min(200) as u16,
                a["cols"].as_u64().unwrap_or(100).min(500) as u16,
            )?;
            if let Some(height) = a["height"].as_u64() {
                let mut disk = app.disk.lock().unwrap();
                if let Some(w) = disk
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == terminal.workspace)
                {
                    if !w.ui["terminal_heights"].is_object() {
                        w.ui["terminal_heights"] = json!({});
                    }
                    w.ui["terminal_heights"][&terminal.id] = json!(height.clamp(180, 4000));
                }
            }
            json!({"ok":true})
        }
        "terminal_remove" => {
            app.terminals.remove(string(&a, "terminal_id")?)?;
            json!({"ok":true})
        }
        "tool" => {
            let name = string(&a, "name")?;
            let arguments = &a["arguments"];
            let mut result =
                crate::tools::execute(app.clone(), string(&a, "workspace")?, name, arguments)
                    .await?;
            strip_api_screenshot_image(name, arguments, &mut result);
            result
        }
        "tool_definitions" => json!(crate::tools::definitions()),
        _ => return Err(err("Unknown action").into()),
    };
    if !matches!(
        action.as_str(),
        "browse"
            | "git_status"
            | "git_file"
            | "git_branch"
            | "codex_usage"
            | "tool_definitions"
            | "service_check_open"
    ) {
        app.save()?;
    }
    Ok(Json(result))
}
async fn media(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(a): Json<Value>,
) -> std::result::Result<Response, ApiError> {
    let w = app.workspace(&id)?;
    let p = crate::context::resolve(&w.path, string(&a, "path")?, false)?;
    if p.metadata()?.len() > 128 * 1024 * 1024 {
        return Err(err("Preview limit is 128 MiB").into());
    }
    let mime = crate::context::mime(&p);
    let data = std::fs::read(p)?;
    Ok(([(header::CONTENT_TYPE, mime)], data).into_response())
}
async fn audio(
    State(app): State<Arc<App>>,
    Path(action): Path<String>,
    Json(a): Json<Value>,
) -> std::result::Result<Response, ApiError> {
    let key = app.key("openai")?;
    let r=match action.as_str(){
        "speech"=>app.client.post("https://api.openai.com/v1/audio/speech").bearer_auth(key).json(&json!({"model":a["model"].as_str().unwrap_or("gpt-4o-mini-tts"),"input":string(&a,"text")?,"voice":a["voice"].as_str().unwrap_or("alloy"),"response_format":"mp3"})).send().await?,
        "transcribe"=>{let bytes=base64::engine::general_purpose::STANDARD.decode(string(&a,"data")?)?;let part=reqwest::multipart::Part::bytes(bytes).file_name(a["filename"].as_str().unwrap_or("audio.webm").to_owned());app.client.post("https://api.openai.com/v1/audio/transcriptions").bearer_auth(key).multipart(reqwest::multipart::Form::new().text("model",a["model"].as_str().unwrap_or("gpt-4o-mini-transcribe").to_owned()).part("file",part)).send().await?},
        "realtime"=>{
            let session=json!({"type":"realtime","model":a["model"].as_str().unwrap_or("gpt-realtime"),"instructions":crate::provider::SYSTEM,"tools":crate::tools::definitions().iter().map(|d|json!({"type":"function","name":d["name"],"description":d["description"],"parameters":d["parameters"]})).collect::<Vec<_>>()});
            app.client.post("https://api.openai.com/v1/realtime/calls").bearer_auth(key).multipart(reqwest::multipart::Form::new().text("sdp",string(&a,"sdp")?.to_owned()).text("session",session.to_string())).send().await?
        },_=>return Err(err("Unknown audio action").into())
    };
    let r = crate::provider::checked(r).await?;
    let mime = r
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    Ok(([(header::CONTENT_TYPE, mime)], r.bytes().await?).into_response())
}
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}

async fn mcp_info(State(app): State<Arc<App>>) -> Response {
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let tools = crate::mcp::handle(app, request)
        .await
        .and_then(|value| value["result"]["tools"].as_array().cloned())
        .unwrap_or_default();
    let mut body = String::from("OK\n");
    body.push_str("<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Lessagent MCP tools</title>");
    body.push_str("<style>body{font:14px/1.5 -apple-system,BlinkMacSystemFont,sans-serif;max-width:980px;margin:24px auto;padding:0 18px;color:#26352f;background:#f8faf7}h1{font-size:22px}p{color:#68746c}details{background:white;border:1px solid #dde4dc;border-radius:8px;margin:9px 0;padding:9px 12px}summary{cursor:pointer;font-weight:650}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#f3f6f2;padding:10px;border-radius:6px}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}</style>");
    body.push_str("<main><h1>Lessagent MCP tools</h1><p>Click a tool name to show its description, input schema, and annotations.</p>");
    for tool in tools {
        let name = html_escape(tool["name"].as_str().unwrap_or("tool"));
        let description = html_escape(tool["description"].as_str().unwrap_or(""));
        let schema = html_escape(
            &serde_json::to_string_pretty(&tool["inputSchema"]).unwrap_or_else(|_| "{}".into()),
        );
        let annotations = html_escape(
            &serde_json::to_string_pretty(&tool["annotations"]).unwrap_or_else(|_| "{}".into()),
        );
        body.push_str(&format!("<details><summary><code>{name}</code></summary><p>{description}</p><strong>Input schema</strong><pre>{schema}</pre><strong>Annotations</strong><pre>{annotations}</pre></details>\n"));
    }
    body.push_str("</main>");
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        body,
    )
        .into_response()
}

async fn mcp(State(app): State<Arc<App>>, Json(v): Json<Value>) -> Response {
    match crate::mcp::handle(app, v).await {
        Some(v) => Json(v).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}
pub async fn serve(app: Arc<App>, port: u16) -> Result<()> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await?;
    let port = listener.local_addr()?.port();
    app.terminals
        .set_connection(port, app.dir.clone(), std::env::current_exe()?);
    println!(
        "Listening on all IPv4 interfaces (0.0.0.0:{port}); remote browser: http://<server-address>:{port}/\nApp HTTP port: {port}\nMCP HTTP port: {port} — http://127.0.0.1:{port}/mcp"
    );
    println!(
        "Lessagent is running. Open http://127.0.0.1:{port}/\nKeep this process running; closing the browser does not stop jobs."
    );
    let discovery = app.clone();
    tokio::spawn(async move {
        let provider = discovery.disk.lock().unwrap().settings.provider.clone();
        match crate::provider::models(discovery.clone(), &provider).await {
            Ok(models) => discovery.log(
                "models",
                &format!(
                    "Startup discovery: {} {} models",
                    models.as_array().map_or(0, |m| m.len()),
                    provider
                ),
            ),
            Err(e) => discovery.log("models", &format!("Startup discovery unavailable: {e}")),
        }
    });
    let shutdown = app.clone();
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = shutdown.shutdown.notified() => {} }
        for c in shutdown.cancellations.lock().unwrap().values() {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        shutdown.terminals.shutdown();
        let _ = shutdown.save();
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_api_screenshot_image;
    use serde_json::json;

    #[test]
    fn normal_api_screenshot_keeps_path_but_drops_image_bytes() {
        let mut result = json!({
            "path": "agent/output/screenshot.png",
            "mime": "image/png",
            "screen_width": 2560,
            "screen_height": 1600,
            "image": {"mime": "image/png", "data": "very-long-base64-payload"}
        });

        strip_api_screenshot_image("computer", &json!({"action": "screenshot"}), &mut result);

        assert_eq!(result["path"], "agent/output/screenshot.png");
        assert_eq!(result["screen_width"], 2560);
        assert!(result.get("image").is_none());
    }

    #[test]
    fn automatic_observations_drop_binary_only_at_browser_api_boundary() {
        let result = json!({"ok":true,"automatic_screenshot":true,"path":"output/click.png",
            "image":{"mime":"image/png","data":"large-base64"},"window_id":5});
        for name in [
            "computer",
            "virtual_pointer",
            "virtual_keyboard",
            "app_open",
        ] {
            let mut value = result.clone();
            strip_api_screenshot_image(name, &json!({"action":"click"}), &mut value);
            assert!(value.get("image").is_none(), "{name}: {value}");
            assert_eq!(value["path"], "output/click.png");
            assert_eq!(value["window_id"], 5);
        }
        let mut screenshot = result.clone();
        screenshot
            .as_object_mut()
            .unwrap()
            .remove("automatic_screenshot");
        strip_api_screenshot_image("get_screenshot", &json!({}), &mut screenshot);
        assert!(screenshot.get("image").is_none());
        let mut opened = json!({"automatic_screenshot":true,"path":"output/browser.png",
            "image":{"mime":"image/png","data":"large-base64"},"pid":8});
        strip_api_screenshot_image(
            "browser_open",
            &json!({"url":"http://127.0.0.1:4173/"}),
            &mut opened,
        );
        assert!(opened.get("image").is_none());
        assert_eq!(opened["pid"], 8);
    }

    #[test]
    fn non_screenshot_results_are_unchanged() {
        let mut result = json!({"ok": true, "image": {"data": "keep-this"}});
        strip_api_screenshot_image("computer", &json!({"action": "click"}), &mut result);
        assert_eq!(result["image"]["data"], "keep-this");
    }
}
