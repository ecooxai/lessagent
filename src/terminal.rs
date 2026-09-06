use crate::{Result, err, state::atomic_write};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const OUTPUT_LIMIT: usize = 256 * 1024;
struct Runtime {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}
pub struct Terminal {
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub created: u64,
    pub managed: bool,
    command: bool,
    runtime: Option<Runtime>,
    screen: Mutex<vt100::Parser>,
    raw: Mutex<Vec<u8>>,
    offset: AtomicU64,
    pub cwd: Mutex<PathBuf>,
    pub exited: AtomicBool,
    exit_code: Mutex<Option<u32>>,
}
impl Terminal {
    pub fn snapshot(&self) -> Value {
        let running = !self.exited.load(Ordering::SeqCst)
            && (self.command || self.runtime.as_ref().is_some_and(|rt| {
                let pid = rt.child.lock().unwrap().process_id();
                let group = rt.master.lock().unwrap().process_group_leader();
                matches!((pid, group), (Some(pid), Some(group)) if group > 0 && group as u32 != pid)
            }));
        let screen = self.screen.lock().unwrap();
        json!({"id":self.id,"workspace":self.workspace,"title":self.title,"created":self.created,"managed":self.managed,"cwd":*self.cwd.lock().unwrap(),"running":running,"screen":screen.screen().contents(),"cursor":screen.screen().cursor_position(),"exited":self.exited.load(Ordering::SeqCst),"exit_code":*self.exit_code.lock().unwrap()})
    }
    pub fn view(&self, revision: Option<u64>, scrollback: usize) -> Value {
        let mut parser = self.screen.lock().unwrap();
        let current = self.offset.load(Ordering::SeqCst);
        if revision == Some(current) && scrollback == 0 {
            return json!({"unchanged":true,"exited":self.exited.load(Ordering::SeqCst)});
        }
        parser.screen_mut().set_scrollback(scrollback.min(1000));
        let screen = parser.screen();
        let (height, width) = screen.size();
        let mut rows = Vec::new();
        for r in 0..height {
            let mut cells = Vec::new();
            for c in 0..width {
                if let Some(cell) = screen.cell(r, c) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let text = cell.contents();
                    cells.push(json!([
                        if text.is_empty() { " " } else { text },
                        format!("{:?}", cell.fgcolor()),
                        format!("{:?}", cell.bgcolor()),
                        cell.bold(),
                        cell.italic(),
                        cell.underline(),
                        cell.inverse()
                    ]));
                }
            }
            rows.push(cells);
        }
        let value = json!({"revision":current,"rows":rows,"cursor":screen.cursor_position(),"hide_cursor":screen.hide_cursor(),"application_cursor":screen.application_cursor(),"bracketed_paste":screen.bracketed_paste(),"scrollback":screen.scrollback(),"exited":self.exited.load(Ordering::SeqCst)});
        parser.screen_mut().set_scrollback(0);
        value
    }
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.raw.lock().unwrap()).into()
    }
    pub fn write(&self, input: &str) -> Result<()> {
        if self.exited.load(Ordering::SeqCst) {
            return Err(err("Terminal has exited"));
        }
        let rt = self
            .runtime
            .as_ref()
            .ok_or_else(|| err("Terminal is an archived session"))?;
        let mut writer = rt.writer.lock().unwrap();
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if !(2..=200).contains(&rows) || !(10..=500).contains(&cols) {
            return Err(err("Invalid terminal dimensions"));
        }
        if let Some(rt) = &self.runtime {
            rt.master.lock().unwrap().resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        self.screen
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
        self.offset.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    pub fn kill(&self) -> Result<()> {
        if let Some(rt) = &self.runtime {
            if self.exited.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Stop the foreground job as well as its shell. PTY children run in
            // their own process group; never signal the backend's group.
            let mut child = rt.child.lock().unwrap();
            if child.try_wait()?.is_none() {
                unsafe extern "C" {
                    fn kill(pid: i32, signal: i32) -> i32;
                }
                if let Some(group) = rt.master.lock().unwrap().process_group_leader()
                    && group > 1
                {
                    unsafe {
                        kill(-group, 9);
                    }
                }
                if let Some(pid) = child.process_id()
                    && pid > 1
                {
                    unsafe {
                        kill(-(pid as i32), 9);
                    }
                }
                child.kill()?;
            }
        }
        Ok(())
    }
}
pub struct TerminalManager {
    sessions: Mutex<HashMap<String, Arc<Terminal>>>,
    dir: PathBuf,
}
impl TerminalManager {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let mut sessions = HashMap::new();
        for e in std::fs::read_dir(&dir)? {
            let p = e?.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let m: Value = serde_json::from_slice(&std::fs::read(&p)?)?;
            let id = m["id"]
                .as_str()
                .ok_or_else(|| err("Invalid terminal metadata"))?
                .to_owned();
            let raw = std::fs::read(dir.join(format!("{id}.log"))).unwrap_or_default();
            let mut screen = vt100::Parser::new(24, 100, 1000);
            screen.process(&raw);
            sessions.insert(
                id.clone(),
                Arc::new(Terminal {
                    id,
                    workspace: m["workspace"].as_str().unwrap_or_default().into(),
                    title: m["title"].as_str().unwrap_or("Previous session").into(),
                    created: m["created"].as_u64().unwrap_or(0),
                    managed: m["managed"].as_bool().unwrap_or(false),
                    command: false,
                    runtime: None,
                    screen: Mutex::new(screen),
                    offset: AtomicU64::new(raw.len() as u64),
                    cwd: Mutex::new(PathBuf::from(m["cwd"].as_str().unwrap_or("/"))),
                    raw: Mutex::new(raw),
                    exited: AtomicBool::new(true),
                    exit_code: Mutex::new(None),
                }),
            );
        }
        Ok(Self {
            sessions: Mutex::new(sessions),
            dir,
        })
    }
    pub fn get(&self, id: &str) -> Result<Arc<Terminal>> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| err("Terminal not found"))
    }
    pub fn list(&self) -> Vec<Value> {
        let mut list: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|t| t.snapshot())
            .collect();
        list.sort_by_key(|a| {
            (
                a["created"].as_u64().unwrap_or(0),
                a["id"].as_str().unwrap_or_default().to_owned(),
            )
        });
        list
    }
    pub fn spawn(
        &self,
        workspace: &str,
        cwd: &Path,
        command: Option<&str>,
    ) -> Result<Arc<Terminal>> {
        self.spawn_owned(workspace, cwd, command, false)
    }
    pub fn spawn_owned(
        &self,
        workspace: &str,
        cwd: &Path,
        command: Option<&str>,
        managed: bool,
    ) -> Result<Arc<Terminal>> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions
            .values()
            .filter(|t| !t.exited.load(Ordering::SeqCst))
            .count()
            >= 32
        {
            return Err(err("At most 32 running terminals"));
        }
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new("/bin/bash");
        if let Some(command) = command {
            cmd.args(["--noprofile", "--norc", "-c", command]);
        } else {
            cmd.args(["--noprofile", "--norc", "-i"]);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("PS1", "\\w $ ");
        // Provider and backend credentials must not be inherited by arbitrary commands.
        for key in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "LESSAGENT_TOKEN",
        ] {
            cmd.env_remove(key);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let t = Arc::new(Terminal {
            id: crate::id(),
            workspace: workspace.into(),
            title: crate::clip(command.unwrap_or("bash"), 80),
            created: crate::now(),
            managed,
            command: command.is_some(),
            runtime: Some(Runtime {
                master: Mutex::new(pair.master),
                writer: Mutex::new(writer),
                child: Mutex::new(child),
            }),
            screen: Mutex::new(vt100::Parser::new(24, 100, 1000)),
            raw: Mutex::new(Vec::new()),
            offset: AtomicU64::new(0),
            cwd: Mutex::new(cwd.to_path_buf()),
            exited: AtomicBool::new(false),
            exit_code: Mutex::new(None),
        });
        atomic_write(
            &self.dir.join(format!("{}.json", t.id)),
            &serde_json::to_vec(
                &json!({"id":t.id,"workspace":workspace,"title":t.title,"created":t.created,"managed":managed,"cwd":cwd}),
            )?,
        )?;
        sessions.insert(t.id.clone(), t.clone());
        drop(sessions);
        let terminal = t.clone();
        let log = self.dir.join(format!("{}.log", t.id));
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        terminal.screen.lock().unwrap().process(&buf[..n]);
                        let mut raw = terminal.raw.lock().unwrap();
                        raw.extend_from_slice(&buf[..n]);
                        terminal.offset.fetch_add(n as u64, Ordering::SeqCst);
                        if raw.len() > OUTPUT_LIMIT {
                            let excess = raw.len() - OUTPUT_LIMIT;
                            raw.drain(..excess);
                        }
                        let _ = atomic_write(&log, &raw);
                    }
                }
            }
            if let Some(rt) = &terminal.runtime {
                loop {
                    match rt.child.lock().unwrap().try_wait() {
                        Ok(Some(status)) => {
                            *terminal.exit_code.lock().unwrap() = Some(status.exit_code());
                            break;
                        }
                        Err(_) => break,
                        Ok(None) => {}
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            terminal.exited.store(true, Ordering::SeqCst);
        });
        Ok(t)
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        let terminal = self.get(id)?;
        if !terminal.exited.load(Ordering::SeqCst) {
            return Err(err("Stop terminal before removing it"));
        }
        self.sessions.lock().unwrap().remove(id);
        for ext in ["json", "log"] {
            let p = self.dir.join(format!("{id}.{ext}"));
            if p.exists() {
                std::fs::remove_file(p)?;
            }
        }
        Ok(())
    }
    pub fn shutdown(&self) {
        for t in self.sessions.lock().unwrap().values() {
            if !t.exited.load(Ordering::SeqCst) {
                let _ = t.kill();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deep_scrollback_survives_resize_and_returns_to_live_screen() {
        let dir = std::env::temp_dir().join(crate::id());
        let m = TerminalManager::new(dir.clone()).unwrap();
        let t = m
            .spawn(
                "test",
                Path::new("/tmp"),
                Some("for i in {1..150}; do echo line-$i; done"),
            )
            .unwrap();
        for _ in 0..100 {
            if t.exited.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(t.exited.load(Ordering::SeqCst));
        let live = t.view(None, 0);
        let history = t.view(None, 80);
        assert_eq!(history["scrollback"], 80);
        assert_ne!(history["rows"], live["rows"]);
        t.resize(10, 120).unwrap();
        for offset in [0, 3, 40, 1000, usize::MAX, 0] {
            let view = t.view(None, offset);
            assert_eq!(view["rows"].as_array().unwrap().len(), 10);
            assert!(t.snapshot()["screen"].is_string());
        }
        assert_eq!(t.view(None, 0)["scrollback"], 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn pty_runs_and_retains_output() {
        let dir = std::env::temp_dir().join(crate::id());
        let m = TerminalManager::new(dir.clone()).unwrap();
        let t = m
            .spawn_owned(
                "test",
                Path::new("/tmp"),
                Some("printf 'hello\\n'; exit 7"),
                true,
            )
            .unwrap();
        for _ in 0..100 {
            if t.exited.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(t.exited.load(Ordering::SeqCst));
        assert!(t.output().contains("hello"));
        assert_eq!(t.snapshot()["exit_code"], 7);
        assert_eq!(t.snapshot()["managed"], true);
        assert_eq!(t.snapshot()["running"], false);
        let view = t.view(None, 0);
        assert!(view["rows"].is_array());
        assert_eq!(t.view(view["revision"].as_u64(), 0)["unchanged"], true);
        t.resize(30, 80).unwrap();
        assert_eq!(
            t.view(view["revision"].as_u64(), 0)["rows"]
                .as_array()
                .unwrap()
                .len(),
            30
        );
        let restored = TerminalManager::new(dir.clone()).unwrap();
        assert!(restored.get(&t.id).unwrap().output().contains("hello"));
        assert_eq!(restored.get(&t.id).unwrap().snapshot()["managed"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
