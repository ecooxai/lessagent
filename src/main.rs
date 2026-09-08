use lessagent::{Result, err};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
fn data_dir() -> std::path::PathBuf {
    std::env::var_os("LESSAGENT_DATA_DIR")
        .map(Into::into)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                        .join(".local/share")
                })
                .join("lessagent")
        })
}
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("lessagent: {e}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut port = std::env::var("LESSAGENT_PORT")
        .unwrap_or("3210".into())
        .parse::<u16>()?;
    let mut dir = data_dir();
    let mut password = None;
    for flag in ["--port", "--data-dir", "--passwd"] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            if i + 1 >= args.len() {
                return Err(err(format!("{flag} requires a value")));
            }
            let value = args.remove(i + 1);
            args.remove(i);
            if flag == "--port" {
                port = value.parse()?;
            } else if flag == "--passwd" {
                if value.is_empty() {
                    return Err(err("Password must not be empty"));
                }
                password = Some(value);
            } else {
                dir = value.into();
            }
        }
    }
    let command = args.first().map(String::as_str).unwrap_or("launch");
    if ["help", "--help", "-h"].contains(&command) {
        println!(
            "Lessagent — persistent local computer agent\n\n  lessagent [start|tui|restart] [--port 3210] [--data-dir DIR] [--passwd PASSWORD]\n  lessagent stop [--port 3210] [--data-dir DIR]\n  lessagent serve [--port 3210] [--data-dir DIR] [--passwd PASSWORD]\n  lessagent open PATH\n  lessagent run PATH PROMPT...\n  lessagent status\n  lessagent models codex|openai|gemini|claude\n  lessagent tool WORKSPACE_ID NAME JSON_ARGUMENTS\n  lessagent mcp\n\nRun lessagent to start and choose CLI, browser UI, or neither. Clients use the same --port and --data-dir (or LESSAGENT_PORT / LESSAGENT_DATA_DIR). API keys: OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY. Codex uses codex login file credentials for direct HTTP requests."
        );
        return Ok(());
    }
    if command == "stop" {
        if password.is_some() {
            return Err(err(
                "Use --passwd with start, restart, tui, serve, or no command",
            ));
        }
        return stop_backend(&dir, port).await;
    }
    if command == "restart" {
        stop_backend(&dir, port).await?;
    }
    if ["launch", "start", "tui", "restart"].contains(&command) {
        return launch(dir, port, password, command).await;
    }
    if command == "serve" {
        let app = lessagent::state::App::load(dir)?;
        if let Some(password) = password {
            app.set_password(&password)?;
        }
        return lessagent::server::serve(app, port).await;
    }
    if password.is_some() {
        return Err(err(
            "Use --passwd with start, restart, tui, serve, or no command",
        ));
    }
    let token = std::fs::read_to_string(dir.join("token"))
        .map_err(|_| err("Start lessagent serve first (use the same data directory)"))?;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    if command == "mcp" {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(request) => {
                    let id = request.get("id").cloned();
                    match client
                        .post(format!("{base}/mcp"))
                        .bearer_auth(&token)
                        .json(&request)
                        .send()
                        .await
                    {
                        Ok(r) if r.status() == 202 => None,
                        Ok(r) if r.status().is_success() => Some(r.json::<Value>().await?),
                        Ok(r) => Some(
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":r.text().await?}}),
                        ),
                        Err(e) => Some(
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":e.to_string()}}),
                        ),
                    }
                }
                Err(_) => Some(
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
                ),
            };
            if let Some(v) = response {
                stdout.write_all(format!("{v}\n").as_bytes()).await?;
                stdout.flush().await?;
            }
        }
        return Ok(());
    }
    let api = |action: &str, body: Value| {
        client
            .post(format!("{base}/api/action/{action}"))
            .bearer_auth(&token)
            .json(&body)
    };
    let result = match command {
        "status" => {
            lessagent::provider::checked(
                client
                    .get(format!("{base}/api/state"))
                    .bearer_auth(&token)
                    .send()
                    .await?,
            )
            .await?
            .json::<Value>()
            .await?
        }
        "models" => {
            lessagent::provider::checked(
                client
                    .get(format!(
                        "{base}/api/models/{}",
                        args.get(1).ok_or_else(|| err("Specify provider"))?
                    ))
                    .bearer_auth(&token)
                    .send()
                    .await?,
            )
            .await?
            .json()
            .await?
        }
        "open" | "run" => {
            let path =
                std::path::Path::new(args.get(1).ok_or_else(|| err("Specify workspace path"))?)
                    .canonicalize()?;
            let w: Value = lessagent::provider::checked(
                api("workspace_open", json!({"path":path})).send().await?,
            )
            .await?
            .json()
            .await?;
            if command == "open" {
                w
            } else {
                let prompt = args.get(2..).unwrap_or_default().join(" ");
                let j: Value = lessagent::provider::checked(
                    api("run", json!({"workspace":w["id"],"prompt":prompt}))
                        .send()
                        .await?,
                )
                .await?
                .json()
                .await?;
                loop {
                    tokio::select! {_ = tokio::signal::ctrl_c()=>{let _=api("stop",j.clone()).send().await;return Err(err("Task cancellation requested"));},_ = tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
                    let s: Value = lessagent::provider::checked(
                        client
                            .get(format!("{base}/api/state"))
                            .bearer_auth(&token)
                            .send()
                            .await?,
                    )
                    .await?
                    .json()
                    .await?;
                    let job = s["jobs"]
                        .as_array()
                        .and_then(|jobs| jobs.iter().find(|x| x["id"] == j["job_id"]))
                        .ok_or_else(|| err("Job disappeared"))?;
                    if job["status"] != "running" {
                        println!("{}", job["output"].as_str().unwrap_or(""));
                        eprintln!("{}", lessagent::provider::metrics(job));
                        if job["status"] != "completed" {
                            return Err(err(format!("Job {}", job["status"])));
                        }
                        return Ok(());
                    }
                }
            }
        }
        "tool" => {
            if args.len() != 4 {
                return Err(err(
                    "Usage: lessagent tool WORKSPACE_ID NAME JSON_ARGUMENTS",
                ));
            }
            lessagent::provider::checked(api("tool",json!({"workspace":args[1],"name":args[2],"arguments":serde_json::from_str::<Value>(&args[3])?})).send().await?).await?.json().await?
        }
        _ => return Err(err("Unknown command; run lessagent --help")),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn launch(
    dir: std::path::PathBuf,
    port: u16,
    password: Option<String>,
    mode: &str,
) -> Result<()> {
    if port == 0 {
        return Err(err(
            "Use a fixed port for start/tui; --port 0 is supported by serve",
        ));
    }
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let ready = || async {
        let token = std::fs::read_to_string(dir.join("token")).ok()?;
        let response = client
            .get(format!("{base}/api/state"))
            .bearer_auth(&token)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let state = response.json::<Value>().await.ok()?;
        (state["workspaces"].is_array() && state["settings"].is_object()).then_some(token)
    };
    let mut existing = ready().await;
    if existing.is_some() {
        let health: Value = client
            .get(format!("{base}/health"))
            .send()
            .await?
            .json()
            .await?;
        if health["management_api"] != 1 {
            println!("Upgrading the running backend to this version.");
            stop_backend(&dir, port).await?;
            existing = None;
        }
    }
    let token = if let Some(token) = existing {
        if let Some(password) = &password {
            lessagent::provider::checked(
                client
                    .post(format!("{base}/api/admin/password"))
                    .bearer_auth(&token)
                    .json(&json!({"password":password}))
                    .send()
                    .await?,
            )
            .await?;
            println!("Browser password updated.");
        }
        println!("Connecting to running Lessagent.");
        token
    } else {
        std::fs::create_dir_all(&dir)?;
        let log_path = dir.join("server.log");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        // Detach the backend from the TUI's process group so Ctrl-C only detaches the UI.
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .args(["serve", "--port", &port.to_string(), "--data-dir"])
            .arg(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some(password) = &password {
            command.args(["--passwd", password]);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        println!("Backend PID: {}", child.id());
        let mut token = None;
        for _ in 0..50 {
            if let Some(value) = ready().await {
                token = Some(value);
                break;
            }
            if let Some(status) = child.try_wait()? {
                return Err(err(format!(
                    "Backend exited ({status}); see {}",
                    log_path.display()
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        println!("Backend log: {}", log_path.display());
        token.ok_or_else(|| {
            err(format!(
                "Backend startup timed out; see {}",
                log_path.display()
            ))
        })?
    };
    println!(
        "Remote browser: http://<server-address>:{port}/ (password or token required)\nApp HTTP port: {port} — {base}/\nMCP HTTP port: {port} — {base}/mcp"
    );
    if mode == "tui" {
        return lessagent::tui::run(&base, &token).await;
    }
    if mode != "launch" {
        return Ok(());
    }
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        println!("Backend ready. Use lessagent tui for CLI, or open {base}/ in a browser.");
        return Ok(());
    }
    loop {
        print!("Open [c] CLI, [b] browser UI, or [n] neither? [n]: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "c" | "cli" | "tui" | "1" => return lessagent::tui::run(&base, &token).await,
            "b" | "browser" | "2" => {
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                };
                let status = std::process::Command::new(opener)
                    .arg(format!("{base}/"))
                    .status()?;
                if !status.success() {
                    return Err(err(format!(
                        "Could not open browser; open {base}/ manually"
                    )));
                }
                return Ok(());
            }
            "" | "n" | "no" | "neither" | "3" => return Ok(()),
            _ => println!("Choose c, b, or n."),
        }
    }
}

async fn stop_backend(dir: &std::path::Path, port: u16) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let base = format!("http://127.0.0.1:{port}");
    match client.get(format!("{base}/health")).send().await {
        Err(e) if e.is_connect() => {
            println!("Lessagent is already stopped on port {port}.");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
        Ok(response) => {
            let health: Value = lessagent::provider::checked(response).await?.json().await?;
            if health["service"] != "lessagent" {
                return Err(err("This port is not running Lessagent"));
            }
        }
    }
    let token = std::fs::read_to_string(dir.join("token"))?;
    let response = client
        .post(format!("{base}/api/admin/shutdown"))
        .bearer_auth(&token)
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Old versions have no shutdown endpoint. Authenticate before identifying
        // the process by BOTH its listener and this data directory's open lock.
        lessagent::provider::checked(
            client
                .get(format!("{base}/api/state"))
                .bearer_auth(&token)
                .send()
                .await?,
        )
        .await?;
        stop_legacy_backend(dir, port)?;
    } else {
        lessagent::provider::checked(response).await?;
    }
    for _ in 0..100 {
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("server.lock"))?;
        if lock.try_lock().is_ok() {
            println!("Lessagent stopped on port {port}.");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(err("Shutdown is still in progress; try again shortly"))
}

fn stop_legacy_backend(dir: &std::path::Path, port: u16) -> Result<()> {
    fn pids(args: &[&std::ffi::OsStr]) -> Result<std::collections::HashSet<u32>> {
        let output = std::process::Command::new("lsof")
            .args(args)
            .output()
            .map_err(|_| {
                err("Stopping an older backend requires lsof; stop its serve process and retry")
            })?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|s| s.parse().ok())
            .collect())
    }
    let lock = dir.join("server.lock").canonicalize()?;
    let owners = pids(&["-t".as_ref(), lock.as_os_str()])?;
    let endpoint = format!("-iTCP:{port}");
    let listeners = pids(&["-t".as_ref(), endpoint.as_ref(), "-sTCP:LISTEN".as_ref()])?;
    let matches: Vec<_> = owners.intersection(&listeners).copied().collect();
    if matches.len() != 1 || matches[0] == std::process::id() {
        return Err(err(
            "Cannot identify the older backend safely; stop its serve process and retry",
        ));
    }
    if !std::process::Command::new("kill")
        .args(["-INT", &matches[0].to_string()])
        .status()?
        .success()
    {
        return Err(err("Could not stop the older backend"));
    }
    println!("Stopping older Lessagent backend (PID {}).", matches[0]);
    Ok(())
}
