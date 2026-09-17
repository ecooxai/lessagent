use crate::{Result, err, terminal::TerminalManager};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub max_steps: usize,
    pub thinking: String,
    pub quality_threshold: f64,
    pub compact_model: String,
    pub computer_enabled: bool,
    pub context_ignores: Vec<String>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "codex".into(),
            model: String::new(),
            max_steps: 24,
            thinking: "medium".into(),
            quality_threshold: 9.,
            compact_model: "gpt-5.6-luna".into(),
            computer_enabled: false,
            context_ignores: crate::context::default_ignores(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub at: u64,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default = "default_session")]
    pub session: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub mode: String,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub ui: Value,
}
pub fn default_session() -> String {
    "chat".into()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub workspace: String,
    #[serde(default = "default_session")]
    pub session: String,
    pub status: String,
    pub prompt: String,
    pub output: String,
    pub events: Vec<Value>,
    pub started: u64,
    #[serde(default)]
    pub usage: Option<crate::provider::Usage>,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub usage_incomplete: bool,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub thinking: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    pub settings: Settings,
    pub workspaces: Vec<Workspace>,
    pub jobs: Vec<Job>,
    pub ui: Value,
    pub logs: Vec<Value>,
}
pub struct App {
    _lock: std::fs::File,
    pub disk: Mutex<Disk>,
    pub dir: PathBuf,
    pub token: String,
    pub password: Mutex<Option<String>>,
    pub shutdown: tokio::sync::Notify,
    pub terminals: TerminalManager,
    pub client: reqwest::Client,
    pub cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub secrets: Mutex<HashMap<String, String>>,
}
impl App {
    pub fn load(dir: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&dir)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        use std::os::unix::fs::OpenOptionsExt;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("server.lock"))?;
        lock.try_lock()
            .map_err(|_| err("Another backend is using this data directory"))?;
        let mut disk: Disk = match std::fs::read(dir.join("state.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Disk::default(),
            Err(e) => return Err(e.into()),
        };
        for job in &mut disk.jobs {
            for event in &mut job.events {
                archive_event(&dir, event)?;
            }

            if job.status == "running" {
                job.status = "interrupted".into();
            }
        }
        let token = match std::fs::read_to_string(dir.join("token")) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let t = crate::id();
                atomic_write(&dir.join("token"), t.as_bytes())?;
                t
            }
            Err(e) => return Err(e.into()),
        };
        let secrets = match std::fs::read(dir.join("keys.json")) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        let app = Arc::new(Self {
            _lock: lock,
            disk: Mutex::new(disk),
            dir: dir.clone(),
            token,
            password: Mutex::new(match std::fs::read_to_string(dir.join("password.hash")) {
                Ok(hash) => Some(hash),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            }),
            shutdown: tokio::sync::Notify::new(),
            terminals: TerminalManager::new(dir.join("terminals"))?,
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .timeout(std::time::Duration::from_secs(180))
                .build()?,
            cancellations: Mutex::new(HashMap::new()),
            secrets: Mutex::new(secrets),
        });
        app.save()?;
        Ok(app)
    }
    pub fn set_password(&self, password: &str) -> Result<()> {
        use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
        if password.is_empty() {
            return Err(err("Password must not be empty"));
        }
        let salt =
            SaltString::encode_b64(crate::id().as_bytes()).map_err(|e| err(e.to_string()))?;
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| err(e.to_string()))?
            .to_string();
        atomic_write(&self.dir.join("password.hash"), hash.as_bytes())?;
        *self.password.lock().unwrap() = Some(hash);
        Ok(())
    }
    pub fn save(&self) -> Result<()> {
        let disk = self.disk.lock().unwrap();
        atomic_write(&self.dir.join("state.json"), &serde_json::to_vec(&*disk)?)
    }
    pub fn workspace(&self, id: &str) -> Result<Workspace> {
        self.disk
            .lock()
            .unwrap()
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .cloned()
            .ok_or_else(|| err("Workspace not found"))
    }
    pub fn open_workspace(&self, path: &Path) -> Result<Workspace> {
        let path = path.canonicalize()?;
        if !path.is_dir() {
            return Err(err("Workspace must be a directory"));
        }
        let mut disk = self.disk.lock().unwrap();
        if let Some(w) = disk.workspaces.iter_mut().find(|w| w.path == path) {
            w.closed = false;
            if !w.ui["tabs"].is_array() {
                w.ui =
                    json!({"tabs":[{"id":"chat","kind":"chat","title":"Agent"}],"active":"chat"});
            }
            for (kind, title) in [
                ("terminals", "Terminal"),
                ("history", "History"),
                ("git", "Git"),
            ] {
                let tabs = w.ui["tabs"].as_array_mut().unwrap();
                if !tabs.iter().any(|t| t["kind"] == kind) {
                    tabs.push(json!({"id":kind,"kind":kind,"title":title}));
                }
            }
            if !self
                .terminals
                .summaries()
                .iter()
                .any(|t| t["workspace"] == w.id && t["exited"] == false)
            {
                self.terminals.spawn(&w.id, &w.path, None)?;
            }
            let w = w.clone();
            drop(disk);
            self.save()?;
            return Ok(w);
        }
        let w = Workspace {
            id: crate::id(),
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            path,
            mode: "normal".into(),
            closed: false,
            messages: vec![],
            ui: json!({"tabs":[{"id":"chat","kind":"chat","title":"Agent"},{"id":"terminals","kind":"terminals","title":"Terminal"},{"id":"history","kind":"history","title":"History"},{"id":"git","kind":"git","title":"Git"}],"active":"chat"}),
        };
        self.terminals.spawn(&w.id, &w.path, None)?;
        disk.workspaces.push(w.clone());
        drop(disk);
        self.save()?;
        Ok(w)
    }
    pub fn log(&self, kind: &str, message: &str) {
        self.log_with_details(kind, message, None);
    }
    pub fn log_with_details(&self, kind: &str, message: &str, details: Option<Value>) {
        self.log_with_details_id(kind, message, details);
    }
    pub fn log_with_details_id(&self, kind: &str, message: &str, details: Option<Value>) -> String {
        let id = crate::id();
        let mut entry =
            json!({"id":id,"at":crate::now(),"kind":kind,"message":crate::clip(message,4000)});
        if let Some(details) = details {
            entry["details"] = details;
        }
        let mut disk = self.disk.lock().unwrap();
        disk.logs.push(entry);
        if disk.logs.len() > 500 {
            disk.logs.remove(0);
        }
        id
    }
    pub fn update_log_details(&self, id: &str, patch: Value) {
        let mut disk = self.disk.lock().unwrap();
        let Some(entry) = disk.logs.iter_mut().find(|entry| entry["id"] == id) else {
            return;
        };
        if !entry["details"].is_object() {
            entry["details"] = json!({});
        }
        let Some(target) = entry["details"].as_object_mut() else {
            return;
        };
        if let Some(patch) = patch.as_object() {
            for (key, value) in patch {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    pub fn event(&self, job: &str, mut event: Value) {
        // Preserve the original event in memory if archiving fails.
        let _ = archive_event(&self.dir, &mut event);
        let mut disk = self.disk.lock().unwrap();
        if let Some(j) = disk.jobs.iter_mut().find(|j| j.id == job) {
            j.events.push(event);
            if j.events.len() > 200
                && let Some(index) = j.events.iter().position(|e| e["kind"] != "request")
            {
                j.events.remove(index);
            }
        }
        drop(disk);
        let _ = self.save();
    }
    pub fn key(&self, provider: &str) -> Result<String> {
        let env = match provider {
            "openai" => "OPENAI_API_KEY",
            "claude" => "ANTHROPIC_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            _ => return Err(err("Unknown API provider")),
        };
        std::env::var(env)
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(|| {
                self.secrets
                    .lock()
                    .unwrap()
                    .get(provider)
                    .cloned()
                    .filter(|k| !k.is_empty())
            })
            .ok_or_else(|| err(format!("Set {env} or add a key in Settings")))
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().ok_or_else(|| err("Missing parent"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.tmp", crate::id()));
    let result = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        if let Ok(metadata) = std::fs::metadata(path) {
            f.set_permissions(metadata.permissions())?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

// Large tool responses belong on disk, not in every UI poll or state clone.
fn archive_event(dir: &Path, event: &mut Value) -> Result<()> {
    if event.get("archived_event").is_some() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(event)?;
    if bytes.len() <= 16 * 1024 {
        return Ok(());
    }
    let id = crate::id();
    let archive = dir.join("events");
    std::fs::create_dir_all(&archive)?;
    atomic_write(&archive.join(format!("{id}.json")), &bytes)?;
    let mut summary = json!({"archived_event":id,"bytes":bytes.len()});
    for key in ["kind", "step", "name", "provider", "model", "language"] {
        if let Some(value) = event.get(key) {
            summary[key] = value.clone();
        }
    }
    *event = summary;
    Ok(())
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    #[test]
    fn large_events_are_preserved_on_disk_and_migration_is_idempotent() {
        let dir = std::env::temp_dir().join(crate::id());
        let original =
            json!({"kind":"light_result","step":2,"result":{"stdout":"x".repeat(100_000)}});
        let mut event = original.clone();
        archive_event(&dir, &mut event).unwrap();
        assert_eq!(event["kind"], "light_result");
        assert!(serde_json::to_vec(&event).unwrap().len() < 256);
        let path = dir.join("events").join(format!(
            "{}.json",
            event["archived_event"].as_str().unwrap()
        ));
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            original
        );
        let summary = event.clone();
        archive_event(&dir, &mut event).unwrap();
        assert_eq!(summary, event);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
