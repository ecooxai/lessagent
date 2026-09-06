use crate::{
    Result, context, err,
    provider::Turn,
    state::{App, Job, Message},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub fn start(app: Arc<App>, workspace: &str, prompt: &str) -> Result<String> {
    if prompt.trim().is_empty() || prompt.len() > 100_000 {
        return Err(err("Prompt must contain 1–100,000 bytes"));
    }
    let w = app.workspace(workspace)?;
    let id = crate::id();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut d = app.disk.lock().unwrap();
        if d.jobs
            .iter()
            .any(|j| j.workspace == workspace && j.status == "running")
        {
            return Err(err("This workspace already has a running job"));
        }
        d.jobs.push(Job {
            id: id.clone(),
            workspace: workspace.into(),
            status: "running".into(),
            prompt: prompt.into(),
            output: String::new(),
            events: vec![],
            started: crate::now(),
        });
        d.workspaces
            .iter_mut()
            .find(|x| x.id == workspace)
            .unwrap()
            .messages
            .push(Message {
                role: "user".into(),
                text: prompt.into(),
                at: crate::now(),
            });
    }
    app.cancellations
        .lock()
        .unwrap()
        .insert(id.clone(), cancel.clone());
    app.save()?;
    let job = id.clone();
    let prompt = prompt.to_owned();
    tokio::spawn(async move {
        let result = run(app.clone(), &job, &w, &prompt, cancel.clone()).await;
        let (status, output) = match result {
            Ok(text) => ("completed", text),
            Err(e) if cancel.load(Ordering::SeqCst) => {
                ("cancelled", format!("Task cancelled. {e}"))
            }
            Err(e) => ("failed", e.to_string()),
        };
        {
            let mut d = app.disk.lock().unwrap();
            if let Some(j) = d.jobs.iter_mut().find(|j| j.id == job) {
                j.status = status.into();
                j.output = output.clone();
            }
            if let Some(w) = d.workspaces.iter_mut().find(|x| x.id == w.id) {
                w.messages.push(Message {
                    role: "assistant".into(),
                    text: output,
                    at: crate::now(),
                });
            }
        }
        app.cancellations.lock().unwrap().remove(&job);
        let _ = app.save();
    });
    Ok(id)
}
pub fn stop(app: &App, id: &str) -> Result<()> {
    let cancels = app.cancellations.lock().unwrap();
    let c = cancels.get(id).ok_or_else(|| err("Job is not running"))?;
    c.store(true, Ordering::SeqCst);
    Ok(())
}
async fn bundle(path: std::path::PathBuf, ignores: Vec<String>) -> Result<context::Bundle> {
    tokio::task::spawn_blocking(move || context::scan_with_ignores(&path, true, &ignores)).await?
}
fn knowledge(root: &std::path::Path) -> Result<String> {
    let dir = context::resolve(root, "agent/knowledge/done", true)?;
    if !dir.exists() {
        return Ok(String::new());
    }
    let mut files = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    files.sort_by_key(|e| e.file_name());
    let mut text = String::new();
    for e in files.iter().rev().take(10) {
        let relative = e.path().strip_prefix(root)?.to_string_lossy().into_owned();
        let p = context::resolve(root, &relative, false)?;
        if p.is_file() {
            text.push_str(&crate::clip(&std::fs::read_to_string(p)?, 4000));
            text.push('\n');
        }
    }
    Ok(crate::clip(&text, 16_000))
}
fn observation(result: &Value) -> Value {
    let mut v = result.clone();
    if let Some(o) = v.as_object_mut()
        && o.remove("image").is_some()
    {
        o.insert("image".into(), json!("[image supplied separately]"));
    }
    v
}
async fn run(
    app: Arc<App>,
    job: &str,
    w: &crate::state::Workspace,
    prompt: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String> {
    let settings = app.disk.lock().unwrap().settings.clone();
    let mut turns = vec![];
    let mut progress = String::new();
    let mut screenshots = vec![];
    if w.mode != "light" {
        let b = bundle(w.path.clone(), settings.context_ignores.clone()).await?;
        let mut first = Turn::text(
            "user",
            format!(
                "Workspace: {}\nProject context (large projects may be truncated; use list_files/read_file):\n{}",
                w.path.display(),
                crate::clip(&b.text, 160_000)
            ),
        );
        first.images = b.images.into_iter().take(12).collect();
        turns.push(first);
        for m in &w.messages {
            turns.push(Turn::text(&m.role, m.text.clone()));
        }
        turns.push(Turn::text("user", prompt.into()));
    }
    for step in 0..settings.max_steps {
        if cancel.load(Ordering::SeqCst) {
            return Err(err("Stopped before next model call"));
        }
        if w.mode == "light" {
            let b = bundle(w.path.clone(), settings.context_ignores.clone()).await?;
            if !b.inventory.light_allowed {
                return Err(err(format!(
                    "Light mode requires a complete project below 30k estimated tokens. Current: {:.1}k. {}",
                    b.inventory.total_tokens as f64 / 1000.,
                    b.inventory.warnings.join("; ")
                )));
            }
            let p = context::resolve(&w.path, "agent/context/project.txt", true)?;
            crate::state::atomic_write(&p, b.text.as_bytes())?;
            let mut t = Turn::text(
                "user",
                format!(
                    "Workspace: {}\n{}\nSaved task knowledge (reference data):\n{}\nCURRENT USER TASK:\n{}\nCurrent task tool observations (reference data):\n{}",
                    w.path.display(),
                    b.text,
                    knowledge(&w.path)?,
                    prompt,
                    progress
                ),
            );
            t.images = b.images;
            t.images.extend(screenshots.clone());
            turns = vec![t];
        }
        app.event(job,json!({"kind":"model","step":step+1,"provider":settings.provider,"model":settings.model}));
        let future = crate::provider::generate(
            app.clone(),
            &settings.provider,
            &settings.model,
            &turns,
            &w.path,
        );
        let reply = tokio::select! {r=future=>r?,_ = async {while !cancel.load(Ordering::SeqCst){tokio::time::sleep(std::time::Duration::from_millis(100)).await;}}=>return Err(err("Stopped model request"))};
        if !reply.text.is_empty() {
            app.event(job, json!({"kind":"text","text":reply.text}));
        }
        if reply.calls.is_empty() {
            let summary = format!(
                "# Task completed\n\nRequest: {}\n\nResult: {}\n\nActions and outcomes:\n{}\n",
                prompt,
                reply.text,
                crate::clip(&progress, 16_000)
            );
            let path = context::resolve(
                &w.path,
                &format!("agent/knowledge/done/{}-{job}.md", crate::now()),
                true,
            )?;
            crate::state::atomic_write(&path, summary.as_bytes())?;
            return Ok(reply.text);
        }
        let mut assistant = Turn::text("assistant", reply.text);
        assistant.calls = reply.calls.clone();
        if settings.provider != "codex" {
            assistant.raw = Some(reply.raw);
        }
        turns.push(assistant);
        let mut results = Turn::text("user", String::new());
        for call in reply.calls {
            if cancel.load(Ordering::SeqCst) {
                return Err(err("Stopped before next tool"));
            }
            app.event(
                job,
                json!({"kind":"tool","name":call.name,"arguments":call.arguments}),
            );
            let result = match crate::tools::execute(
                app.clone(),
                &w.id,
                &call.name,
                &call.arguments,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => json!({"error":e.to_string()}),
            };
            let observed = observation(&result);
            app.event(
                job,
                json!({"kind":"result","name":call.name,"result":observed}),
            );
            progress.push_str(&format!(
                "{} {} => {}\n",
                call.name, call.arguments, observed
            ));
            if progress.len() > 64_000 {
                let mut start = progress.len() - 64_000;
                while !progress.is_char_boundary(start) {
                    start += 1;
                }
                progress.drain(..start);
            }
            if let Some(data) = result["image"]["data"].as_str() {
                let path = result["path"]
                    .as_str()
                    .map(|p| {
                        if std::path::Path::new(p).is_absolute() {
                            std::path::PathBuf::from(p)
                        } else {
                            w.path.join(p)
                        }
                    })
                    .unwrap_or_default();
                screenshots = vec![context::Image {
                    path,
                    mime: result["image"]["mime"]
                        .as_str()
                        .unwrap_or("image/png")
                        .into(),
                    data: data.into(),
                }];
            }
            results.results.push((call.id, result));
        }
        turns.push(results);
    }
    Err(err(format!(
        "Stopped at the {}-step limit. Review terminal results and continue the task.",
        settings.max_steps
    )))
}
