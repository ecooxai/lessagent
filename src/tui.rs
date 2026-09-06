use crate::{Result, provider::checked};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers, MouseEventKind},
    execute,
    terminal::{self, ClearType},
};
use serde_json::{Value, json};
use std::{
    io::{self, IsTerminal, Write},
    time::Duration,
};

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            event::DisableMouseCapture,
            terminal::LeaveAlternateScreen,
            cursor::Show
        );
        let _ = terminal::disable_raw_mode();
    }
}
fn safe(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
pub async fn run(base: &str, token: &str) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        println!("Backend ready. Run lessagent tui in an interactive terminal.");
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    terminal::enable_raw_mode()?;
    let _restore = Restore;
    execute!(
        io::stdout(),
        terminal::EnterAlternateScreen,
        event::EnableMouseCapture,
        cursor::Hide
    )?;
    let mut input = String::new();
    let mut selected = 0usize;
    let mut scroll = 0usize;
    let mut message = String::new();
    loop {
        let state: Value = checked(
            client
                .get(format!("{base}/api/state"))
                .bearer_auth(token)
                .send()
                .await?,
        )
        .await?
        .json()
        .await?;
        let workspaces = state["workspaces"].as_array().cloned().unwrap_or_default();
        selected = selected.min(workspaces.len().saturating_sub(1));
        let workspace = workspaces.get(selected);
        let (width, height) = terminal::size()?;
        let mut lines = Vec::new();
        if let Some(w) = workspace {
            lines.push(format!(
                "Workspace {}/{}: {}",
                selected + 1,
                workspaces.len(),
                w["path"].as_str().unwrap_or("")
            ));
            if let Some(messages) = w["messages"].as_array() {
                for m in messages {
                    lines.push(format!(
                        "{}: {}",
                        m["role"].as_str().unwrap_or(""),
                        m["text"].as_str().unwrap_or("")
                    ));
                }
            }
            if let Some(jobs) = state["jobs"].as_array() {
                for job in jobs.iter().filter(|j| j["workspace"] == w["id"]) {
                    lines.push(format!(
                        "Task [{}]: {}\n{}",
                        job["status"].as_str().unwrap_or(""),
                        job["prompt"].as_str().unwrap_or(""),
                        job["output"].as_str().unwrap_or("")
                    ));
                }
            }
        } else {
            lines.push("No workspaces. Type /open /absolute/project/path".into());
        }
        let lines: Vec<String> = lines
            .iter()
            .flat_map(|line| {
                safe(line)
                    .split('\n')
                    .flat_map(|line| {
                        let chars: Vec<char> = line.chars().collect();
                        if chars.is_empty() {
                            vec![String::new()]
                        } else {
                            chars
                                .chunks(usize::from(width.max(1)))
                                .map(|c| c.iter().collect())
                                .collect()
                        }
                    })
                    .collect::<Vec<String>>()
            })
            .collect();
        let count = usize::from(height.saturating_sub(5));
        scroll = scroll.min(lines.len().saturating_sub(count));
        let start = lines.len().saturating_sub(count + scroll);
        let mut out = io::stdout();
        execute!(out, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;
        let header = format!("Lessagent | {base} | MCP {base}/mcp");
        for (row, line) in std::iter::once(header.as_str())
            .chain(lines.iter().skip(start).take(count).map(String::as_str))
            .enumerate()
        {
            execute!(out, cursor::MoveTo(0, row as u16))?;
            write!(
                out,
                "{}",
                safe(line).chars().take(width as usize).collect::<String>()
            )?;
        }
        for (offset, line) in [
            "Tab: workspace | PgUp/PgDn/wheel: scroll | Esc/Ctrl-C: detach",
            "/open PATH | /stop | /quit | Enter: send task",
            message.as_str(),
            &format!("> {input}"),
        ]
        .iter()
        .enumerate()
        {
            execute!(
                out,
                cursor::MoveTo(0, height.saturating_sub(4) + offset as u16)
            )?;
            write!(
                out,
                "{}",
                safe(line).chars().take(width as usize).collect::<String>()
            )?;
        }
        out.flush()?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        match event::read()? {
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => scroll = scroll.saturating_add(3),
                MouseEventKind::ScrollDown => scroll = scroll.saturating_sub(3),
                _ => {}
            },
            Event::Key(k) if k.kind != event::KeyEventKind::Release => match k.code {
                KeyCode::Esc => break,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Tab => {
                    selected = (selected + 1) % workspaces.len().max(1);
                    scroll = 0;
                }
                KeyCode::PageUp => scroll = scroll.saturating_add(count.max(1)),
                KeyCode::PageDown => scroll = scroll.saturating_sub(count.max(1)),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => input.push(c),
                KeyCode::Enter => {
                    let text = std::mem::take(&mut input);
                    if text.trim() == "/quit" {
                        break;
                    }
                    let action = if let Some(path) = text.strip_prefix("/open ") {
                        Some(("workspace_open", json!({"path":path.trim()})))
                    } else if text.trim() == "/stop" {
                        state["jobs"]
                            .as_array()
                            .and_then(|jobs| {
                                jobs.iter().rev().find(|j| {
                                    j["status"] == "running"
                                        && workspace.is_some_and(|w| j["workspace"] == w["id"])
                                })
                            })
                            .map(|j| ("stop", json!({"job_id":j["id"]})))
                    } else if !text.trim().is_empty() {
                        workspace.map(|w| ("run", json!({"workspace":w["id"],"prompt":text})))
                    } else {
                        None
                    };
                    if let Some((action, body)) = action {
                        let result = async {
                            checked(
                                client
                                    .post(format!("{base}/api/action/{action}"))
                                    .bearer_auth(token)
                                    .json(&body)
                                    .send()
                                    .await?,
                            )
                            .await?
                            .json::<Value>()
                            .await
                            .map_err(crate::Error::from)
                        }
                        .await;
                        message = match result {
                            Ok(_) => {
                                scroll = 0;
                                if action == "workspace_open" {
                                    selected = workspaces.len();
                                }
                                "Done".into()
                            }
                            Err(e) => e.to_string(),
                        };
                    } else {
                        message =
                            "Open a workspace first, or select a running task to stop.".into();
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(())
}
