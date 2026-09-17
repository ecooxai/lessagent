use crate::{Result, err};
use chrono::{Local, TimeZone};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

const DEFAULT_INTERVAL_SECONDS: u64 = 10;
const MIN_INTERVAL_SECONDS: u64 = 1;
const MAX_INTERVAL_SECONDS: u64 = 3600;
const MAX_ERROR_HISTORY: usize = 500;
const MAX_RESPONSE_BYTES: usize = 128 * 1024;
const FAILURE_BG: egui::Color32 = egui::Color32::from_rgb(176, 30, 42);
const ALERT_SIZE: egui::Vec2 = egui::vec2(320.0, 88.0);

#[derive(Default)]
struct MiniAlertState {
    failures: AtomicUsize,
    clicked: AtomicBool,
}

fn mini_alert_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("lessagent-service-mini-alert")
}

fn should_show_mini_alert(main_focused: bool, active_failures: usize) -> bool {
    !main_focused && active_failures > 0
}

fn alert_position(ctx: &egui::Context) -> egui::Pos2 {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        // The first NSScreen is the menu-bar display. Its visibleFrame excludes
        // the menu bar and Dock. Convert AppKit bottom-left to Quartz top-left.
        let screens = objc2_app_kit::NSScreen::screens(mtm);
        if let Some(screen) = screens.firstObject() {
            let full = screen.frame();
            let visible = screen.visibleFrame();
            return egui::pos2(
                (visible.origin.x + visible.size.width) as f32 - ALERT_SIZE.x - 16.0,
                (full.origin.y + full.size.height - visible.origin.y - visible.size.height) as f32
                    + 16.0,
            );
        }
    }
    let size = ctx
        .input(|input| input.viewport().monitor_size)
        .unwrap_or(egui::vec2(1280.0, 800.0));
    egui::pos2((size.x - ALERT_SIZE.x - 16.0).max(0.0), 40.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckKind {
    Api,
    HttpStatus,
    FirstLineOk,
}

#[derive(Clone, Debug)]
struct CheckTarget {
    label: String,
    url: String,
    kind: CheckKind,
    removable: bool,
}

#[derive(Clone, Debug)]
struct CheckResult {
    label: String,
    url: String,
    ok: bool,
    checked_at_ms: u64,
    elapsed_ms: u64,
    http_status: Option<u16>,
    first_line: String,
    headers: String,
    body: String,
    error: Option<String>,
}

impl CheckResult {
    fn detail_text(&self) -> String {
        let status = self
            .http_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unavailable".into());
        let mut detail = format!(
            "GET {}\nChecked: {}\nElapsed: {} ms\nHTTP status: {}\nFirst line: {:?}\nResult: {}\n",
            self.url,
            format_timestamp(self.checked_at_ms),
            self.elapsed_ms,
            status,
            self.first_line,
            if self.ok { "OK" } else { "FAILED" },
        );
        if let Some(error) = &self.error {
            detail.push_str(&format!("Error: {error}\n"));
        }
        detail.push_str("\nResponse headers:\n");
        detail.push_str(if self.headers.is_empty() {
            "(none)"
        } else {
            &self.headers
        });
        detail.push_str("\n\nResponse body:\n");
        detail.push_str(if self.body.is_empty() {
            "(empty)"
        } else {
            &self.body
        });
        detail
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ErrorRecord {
    at_ms: u64,
    label: String,
    url: String,
    detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CheckerConfig {
    #[serde(default = "default_interval_seconds")]
    interval_seconds: u64,
    #[serde(default)]
    custom_urls: Vec<String>,
    #[serde(default)]
    paused_urls: Vec<String>,
    #[serde(default)]
    errors: Vec<ErrorRecord>,
}

impl Default for CheckerConfig {
    fn default() -> Self {
        Self {
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            custom_urls: Vec::new(),
            paused_urls: Vec::new(),
            errors: Vec::new(),
        }
    }
}

fn default_interval_seconds() -> u64 {
    DEFAULT_INTERVAL_SECONDS
}

impl CheckerConfig {
    fn normalize(&mut self) {
        self.interval_seconds = self
            .interval_seconds
            .clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS);
        self.custom_urls.retain(|url| valid_http_url(url));
        self.custom_urls.sort();
        self.custom_urls.dedup();
        self.paused_urls.retain(|url| valid_http_url(url));
        self.paused_urls.sort();
        self.paused_urls.dedup();
        if self.errors.len() > MAX_ERROR_HISTORY {
            self.errors.drain(0..self.errors.len() - MAX_ERROR_HISTORY);
        }
    }
}

pub fn run(data_dir: PathBuf, base_url: String) -> Result<()> {
    let base_url = normalize_base_url(&base_url)?;
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lessagent")
            .with_inner_size([820.0, 720.0])
            .with_min_inner_size([640.0, 480.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Lessagent",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(ServiceCheckerApp::new(
                data_dir.clone(),
                base_url.clone(),
            )))
        }),
    )
    .map_err(|error| err(format!("Service checker GUI failed: {error}")))
}

fn normalize_base_url(input: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(input)
        .map_err(|error| err(format!("Invalid service checker base URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(err("Service checker base URL must use HTTP or HTTPS"));
    }
    Ok(input.trim_end_matches('/').to_owned())
}

fn valid_http_url(input: &str) -> bool {
    reqwest::Url::parse(input.trim())
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn custom_check_kind(input: &str) -> CheckKind {
    reqwest::Url::parse(input.trim())
        .ok()
        .filter(|url| url.path().trim_end_matches('/') == "/mcp")
        .map_or(CheckKind::HttpStatus, |_| CheckKind::FirstLineOk)
}

#[cfg(target_os = "macos")]
fn activate_native_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    if let Some(main_thread) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(main_thread);
        app.activate();
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_native_app() {}

#[cfg(target_os = "macos")]
fn configure_mini_alert() {
    use objc2_app_kit::{NSApplication, NSWindowCollectionBehavior as Behavior};
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        for window in app.windows().iter() {
            if window.title().to_string() == "Lessagent service alert" {
                window.setHidesOnDeactivate(false);
                window.setCanHide(false);
                let mut behavior = Behavior::CanJoinAllSpaces
                    | Behavior::FullScreenAuxiliary
                    | Behavior::Stationary
                    | Behavior::IgnoresCycle;
                if objc2::available!(macos = 13.0) {
                    behavior |= Behavior::CanJoinAllApplications;
                }
                window.setCollectionBehavior(behavior);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_mini_alert() {}

fn open_browser_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");

    let status = command.arg(url).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(err(format!("Could not open browser for {url}")))
    }
}

fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("service-check.json")
}

fn load_config(data_dir: &Path) -> CheckerConfig {
    let mut config: CheckerConfig = std::fs::read(config_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    config.normalize();
    config
}

fn save_config(data_dir: &Path, config: &CheckerConfig) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    crate::state::atomic_write(&config_path(data_dir), &serde_json::to_vec_pretty(config)?)
}

struct ServiceCheckerApp {
    data_dir: PathBuf,
    base_url: String,
    config: CheckerConfig,
    custom_url_input: String,
    input_error: Option<String>,
    results: Vec<CheckResult>,
    selected_url: Option<String>,
    show_errors: bool,
    checking: bool,
    next_check: Instant,
    receiver: Option<Receiver<(u64, Vec<CheckResult>)>>,
    revision: u64,
    cancellation: Option<Arc<AtomicBool>>,
    mini_alert: Arc<MiniAlertState>,
    alert_initialized: bool,
    alert_visible: bool,
}

impl ServiceCheckerApp {
    fn new(data_dir: PathBuf, base_url: String) -> Self {
        let config = load_config(&data_dir);
        Self {
            data_dir,
            base_url,
            config,
            custom_url_input: String::new(),
            input_error: None,
            results: Vec::new(),
            selected_url: None,
            show_errors: false,
            checking: false,
            next_check: Instant::now(),
            receiver: None,
            revision: 0,
            cancellation: None,
            mini_alert: Arc::new(MiniAlertState::default()),
            alert_initialized: false,
            alert_visible: false,
        }
    }

    fn targets(&self) -> Vec<CheckTarget> {
        let mut targets = vec![
            CheckTarget {
                label: "API".into(),
                url: format!("{}/health", self.base_url),
                kind: CheckKind::Api,
                removable: false,
            },
            CheckTarget {
                label: "MCP".into(),
                url: format!("{}/mcp", self.base_url),
                kind: CheckKind::FirstLineOk,
                removable: false,
            },
        ];
        targets.extend(self.config.custom_urls.iter().map(|url| CheckTarget {
            label: if custom_check_kind(url) == CheckKind::FirstLineOk {
                "MCP URL".into()
            } else {
                "HTTP".into()
            },
            url: url.clone(),
            kind: custom_check_kind(url),
            removable: true,
        }));
        targets
    }

    fn launch_checks(&mut self) {
        if self.checking {
            return;
        }
        let targets = self.active_targets();
        self.next_check = Instant::now() + Duration::from_secs(self.config.interval_seconds);
        if targets.is_empty() {
            return;
        }
        let revision = self.revision;
        let cancellation = Arc::new(AtomicBool::new(false));
        self.cancellation = Some(cancellation.clone());
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.checking = true;
        self.next_check = Instant::now() + Duration::from_secs(self.config.interval_seconds);
        std::thread::spawn(move || {
            let results = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(run_checks(targets, cancellation)),
                Err(error) => {
                    failed_checks(targets, &format!("Could not create async runtime: {error}"))
                }
            };
            let _ = sender.send((revision, results));
        });
    }

    fn receive_results(&mut self) {
        let Some(receiver) = &self.receiver else {
            return;
        };
        let (revision, results) = match receiver.try_recv() {
            Ok(batch) => batch,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.cancellation = None;
                self.checking = false;
                self.next_check =
                    Instant::now() + Duration::from_secs(self.config.interval_seconds);
                self.apply_results(failed_checks(
                    self.active_targets(),
                    "Service checker worker stopped before returning a result",
                ));
                return;
            }
        };
        self.receiver = None;
        self.cancellation = None;
        self.checking = false;
        self.next_check = Instant::now() + Duration::from_secs(self.config.interval_seconds);
        // Pause/remove/resume during a request invalidates the batch. A request
        // already sent may finish, but its stale response cannot re-alert or
        // overwrite current state. The next batch includes only active URLs.
        if revision != self.revision {
            self.next_check = Instant::now();
            return;
        }
        self.apply_results(results);
    }

    fn apply_results(&mut self, results: Vec<CheckResult>) {
        let targets = self.targets();
        let mut failed = false;
        for result in results {
            if self.is_paused(&result.url) || !targets.iter().any(|t| t.url == result.url) {
                continue;
            }
            if !result.ok {
                failed = true;
                self.config.errors.push(ErrorRecord {
                    at_ms: result.checked_at_ms,
                    label: result.label.clone(),
                    url: result.url.clone(),
                    detail: result.detail_text(),
                });
            }
            self.results.retain(|previous| previous.url != result.url);
            self.results.push(result);
        }
        self.config.normalize();
        if failed && let Err(error) = save_config(&self.data_dir, &self.config) {
            self.input_error = Some(format!("Could not save failure history: {error}"));
        }
        // Never activate, raise, or request critical attention here. Current
        // active failures drive the mini-alert, not the persisted error history.
    }

    fn active_targets(&self) -> Vec<CheckTarget> {
        self.targets()
            .into_iter()
            .filter(|target| !self.is_paused(&target.url))
            .collect()
    }

    fn invalidate_checks(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(cancellation) = &self.cancellation {
            cancellation.store(true, Ordering::Release);
        }
        self.next_check = Instant::now();
    }

    fn is_paused(&self, url: &str) -> bool {
        self.config.paused_urls.iter().any(|paused| paused == url)
    }

    fn active_failure_count(&self) -> usize {
        self.targets()
            .iter()
            .filter(|target| {
                !self.is_paused(&target.url) && self.result_for(&target.url).is_some_and(|r| !r.ok)
            })
            .count()
    }

    fn sorted_targets(&self) -> Vec<CheckTarget> {
        let mut targets = self.targets();
        targets.sort_by_key(|target| {
            if self.is_paused(&target.url) {
                2
            } else if self.result_for(&target.url).is_some_and(|r| !r.ok) {
                0
            } else {
                1
            }
        });
        targets
    }

    fn toggle_pause(&mut self, url: &str) {
        if !self.targets().iter().any(|target| target.url == url) {
            return;
        }
        let mut config = self.config.clone();
        let resuming = self.is_paused(url);
        if resuming {
            config.paused_urls.retain(|paused| paused != url);
        } else {
            config.paused_urls.push(url.to_owned());
        }
        config.normalize();
        if self.replace_config_on_disk(config, "save pause state") {
            self.invalidate_checks();
            if resuming {
                // Do not show an old failure as current before the resumed check.
                self.results.retain(|result| result.url != url);
            }
            self.next_check = Instant::now();
        }
    }

    fn update_mini_alert(&mut self, ctx: &egui::Context) {
        let count = self.active_failure_count();
        self.mini_alert.failures.store(count, Ordering::Release);
        let clicked = self.mini_alert.clicked.swap(false, Ordering::AcqRel);
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
        self.alert_visible = should_show_mini_alert(focused, count) && !clicked;
        if self.alert_initialized {
            configure_mini_alert();
            ctx.send_viewport_cmd_to(
                mini_alert_id(),
                egui::ViewportCommand::OuterPosition(alert_position(ctx)),
            );
            ctx.send_viewport_cmd_to(
                mini_alert_id(),
                egui::ViewportCommand::Visible(self.alert_visible),
            );
            ctx.request_repaint_of(mini_alert_id());
        }
        if clicked {
            self.selected_url = self
                .sorted_targets()
                .into_iter()
                .find(|t| !self.is_paused(&t.url) && self.result_for(&t.url).is_some_and(|r| !r.ok))
                .map(|t| t.url);
            // Only an explicit click may unhide/focus the full service monitor.
            activate_native_app();
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Minimized(false),
            );
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    }

    fn show_mini_alert(&mut self, ctx: &egui::Context) {
        // Register once from a normal UI pass, including while healthy. Eframe
        // intentionally skips App::ui while the main window is hidden, but
        // App::logic can show/hide this retained deferred child viewport.
        let state = self.mini_alert.clone();
        ctx.show_viewport_deferred(
            mini_alert_id(),
            egui::ViewportBuilder::default()
                .with_title("Lessagent service alert")
                .with_inner_size(ALERT_SIZE)
                .with_position(alert_position(ctx))
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_active(false)
                .with_visible(self.alert_visible)
                .with_always_on_top(),
            move |ui, _class| {
                let count = state.failures.load(Ordering::Acquire);
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(FAILURE_BG).inner_margin(12.0))
                    .show(ui, |ui| {
                        let text = format!(
                            "{} service check{} failed\nClick to open monitor{}",
                            count,
                            if count == 1 { "" } else { "s" },
                            if cfg!(debug_assertions) {
                                "  ·  dev"
                            } else {
                                ""
                            }
                        );
                        if ui
                            .add_sized(
                                ui.available_size(),
                                egui::Button::new(
                                    egui::RichText::new(text)
                                        .color(egui::Color32::WHITE)
                                        .strong()
                                        .size(15.0),
                                )
                                .fill(FAILURE_BG)
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            state.clicked.store(true, Ordering::Release);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                    });
            },
        );
        self.alert_initialized = true;
    }

    fn replace_config_on_disk(&mut self, config: CheckerConfig, action: &str) -> bool {
        match save_config(&self.data_dir, &config) {
            Ok(()) => {
                self.config = config;
                self.input_error = None;
                true
            }
            Err(error) => {
                self.input_error = Some(format!("Could not {action}: {error}"));
                false
            }
        }
    }

    fn add_custom_url(&mut self) {
        let url = self.custom_url_input.trim().to_owned();
        if !valid_http_url(&url) {
            self.input_error = Some("Enter an http:// or https:// URL".into());
            return;
        }
        if self
            .config
            .custom_urls
            .iter()
            .any(|existing| existing == &url)
        {
            self.input_error = Some("That URL is already being checked".into());
            return;
        }
        let mut config = self.config.clone();
        config.custom_urls.push(url);
        config.normalize();
        if self.replace_config_on_disk(config, "save service URL") {
            self.custom_url_input.clear();
            self.invalidate_checks();
            self.next_check = Instant::now();
        }
    }

    fn remove_custom_url(&mut self, url: &str) {
        let mut config = self.config.clone();
        config.custom_urls.retain(|existing| existing != url);
        config.paused_urls.retain(|paused| paused != url);
        if self.replace_config_on_disk(config, "remove service URL") {
            self.invalidate_checks();
            self.results.retain(|result| result.url != url);
            if self.selected_url.as_deref() == Some(url) {
                self.selected_url = None;
            }
            self.next_check = Instant::now();
        }
    }

    fn set_interval(&mut self, seconds: u64) {
        let mut config = self.config.clone();
        config.interval_seconds = seconds.clamp(MIN_INTERVAL_SECONDS, MAX_INTERVAL_SECONDS);
        if self.replace_config_on_disk(config, "save update interval") {
            self.next_check = Instant::now() + Duration::from_secs(self.config.interval_seconds);
        }
    }

    fn result_for(&self, url: &str) -> Option<&CheckResult> {
        self.results.iter().find(|result| result.url == url)
    }
}

impl eframe::App for ServiceCheckerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_results();
        self.update_mini_alert(ctx);
        if !self.checking && Instant::now() >= self.next_check {
            self.launch_checks();
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Lessagent");
                    ui.label("Service monitor");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    #[cfg(debug_assertions)]
                    ui.label(egui::RichText::new("dev").strong().monospace());
                    let label = format!("Errors ({})", self.config.errors.len());
                    if ui.button(label).clicked() {
                        self.show_errors = true;
                    }
                    if ui
                        .add_enabled(!self.checking, egui::Button::new("Check now"))
                        .clicked()
                    {
                        self.launch_checks();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Browser UI:");
                ui.monospace(format!("{}/", self.base_url));
                if ui.button("Open in browser").clicked() {
                    match open_browser_url(&format!("{}/", self.base_url)) {
                        Ok(()) => self.input_error = None,
                        Err(error) => self.input_error = Some(error.to_string()),
                    }
                }
            });
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Update every");
                let mut interval = self.config.interval_seconds;
                if ui
                    .add(
                        egui::DragValue::new(&mut interval)
                            .range(MIN_INTERVAL_SECONDS..=MAX_INTERVAL_SECONDS)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    self.set_interval(interval);
                }
                ui.label(if self.checking { "Checking…" } else { "Idle" });
            });

            ui.label("Add URL");
            ui.horizontal(|ui| {
                let button_width = 92.0;
                let input_width =
                    (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(180.0);
                let response = ui.add_sized(
                    [input_width, 28.0],
                    egui::TextEdit::singleline(&mut self.custom_url_input)
                        .hint_text("https://example.com or https://host.example/mcp"),
                );
                let add_clicked = ui
                    .add_sized([button_width, 28.0], egui::Button::new("Add URL"))
                    .clicked();
                let enter_pressed =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if add_clicked || enter_pressed {
                    self.add_custom_url();
                }
            });
            if let Some(error) = &self.input_error {
                ui.label(error);
            }
            ui.add_space(8.0);

            let targets = self.sorted_targets();
            let mut remove_url: Option<String> = None;
            let mut pause_url: Option<String> = None;
            egui::ScrollArea::vertical()
                .id_salt("service-targets")
                .max_height(330.0)
                .show(ui, |ui| {
                    for target in &targets {
                        let result = self.result_for(&target.url).cloned();
                        let paused = self.is_paused(&target.url);
                        let failed = !paused && result.as_ref().is_some_and(|r| !r.ok);
                        let state = match result.as_ref() {
                            _ if paused => "PAUSED".into(),
                            Some(result) if result.ok => format!("OK · {} ms", result.elapsed_ms),
                            Some(result) => format!("FAILED · {} ms", result.elapsed_ms),
                            None if self.checking => "CHECKING".into(),
                            None => "PENDING".into(),
                        };
                        let frame = egui::Frame::group(ui.style());
                        let frame = if failed {
                            frame.fill(FAILURE_BG)
                        } else {
                            frame
                        };
                        frame.show(ui, |ui| {
                            if failed {
                                let visuals = ui.visuals_mut();
                                visuals.override_text_color = Some(egui::Color32::WHITE);
                                visuals.widgets.inactive.weak_bg_fill =
                                    egui::Color32::from_rgb(125, 16, 28);
                                visuals.widgets.hovered.weak_bg_fill =
                                    egui::Color32::from_rgb(145, 20, 32);
                                visuals.widgets.active.weak_bg_fill =
                                    egui::Color32::from_rgb(104, 12, 24);
                                visuals.selection.bg_fill = egui::Color32::from_rgb(125, 16, 28);
                                visuals.selection.stroke =
                                    egui::Stroke::new(1.0, egui::Color32::WHITE);
                            }
                            ui.horizontal(|ui| {
                                let selected = self.selected_url.as_deref() == Some(&target.url);
                                if ui
                                    .selectable_label(
                                        selected,
                                        format!("{}  —  {}", target.label, state),
                                    )
                                    .clicked()
                                {
                                    self.selected_url = Some(target.url.clone());
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if target.removable && ui.small_button("Remove").clicked() {
                                            remove_url = Some(target.url.clone());
                                        }
                                        // Right-to-left layout puts Pause directly LEFT of Remove.
                                        if ui
                                            .small_button(if paused { "Resume" } else { "Pause" })
                                            .clicked()
                                        {
                                            pause_url = Some(target.url.clone());
                                        }
                                    },
                                );
                            });
                            ui.monospace(&target.url);
                            if let Some(result) = result.as_ref() {
                                ui.small(format!(
                                    "Checked {}",
                                    format_timestamp(result.checked_at_ms)
                                ));
                                if let Some(error) = &result.error {
                                    ui.label(error);
                                }
                            }
                        });
                    }
                });
            if let Some(url) = pause_url {
                self.toggle_pause(&url);
            }
            if let Some(url) = remove_url {
                self.remove_custom_url(&url);
            }

            ui.separator();
            ui.heading("Request result");
            let selected = self
                .selected_url
                .as_deref()
                .and_then(|url| self.result_for(url));
            egui::ScrollArea::vertical()
                .id_salt("request-result")
                .max_height(250.0)
                .show(ui, |ui| match selected {
                    Some(result) => {
                        let mut detail = result.detail_text();
                        ui.add(
                            egui::TextEdit::multiline(&mut detail)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    }
                    None => {
                        ui.label("Click a service row to show its latest full HTTP response.");
                    }
                });
        });

        self.show_mini_alert(&ctx);

        if self.show_errors {
            let mut open = self.show_errors;
            egui::Window::new("Error history")
                .open(&mut open)
                .default_width(720.0)
                .show(&ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} stored failures", self.config.errors.len()));
                        if ui.button("Clear history").clicked() {
                            self.config.errors.clear();
                            let _ = save_config(&self.data_dir, &self.config);
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for error in self.config.errors.iter().rev() {
                            egui::CollapsingHeader::new(format!(
                                "{} · {} · {}",
                                format_timestamp(error.at_ms),
                                error.label,
                                error.url
                            ))
                            .show(ui, |ui| {
                                let mut detail = error.detail.clone();
                                ui.add(
                                    egui::TextEdit::multiline(&mut detail)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            });
                        }
                    });
                });
            self.show_errors = open;
        }
    }
}

fn failed_checks(targets: Vec<CheckTarget>, message: &str) -> Vec<CheckResult> {
    targets
        .into_iter()
        .map(|target| CheckResult {
            label: target.label,
            url: target.url,
            ok: false,
            checked_at_ms: crate::now(),
            elapsed_ms: 0,
            http_status: None,
            first_line: String::new(),
            headers: String::new(),
            body: String::new(),
            error: Some(message.to_owned()),
        })
        .collect()
}

async fn run_checks(targets: Vec<CheckTarget>, cancellation: Arc<AtomicBool>) -> Vec<CheckResult> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return targets
                .into_iter()
                .map(|target| CheckResult {
                    label: target.label,
                    url: target.url,
                    ok: false,
                    checked_at_ms: crate::now(),
                    elapsed_ms: 0,
                    http_status: None,
                    first_line: String::new(),
                    headers: String::new(),
                    body: String::new(),
                    error: Some(format!("Could not build HTTP client: {error}")),
                })
                .collect();
        }
    };
    let mut results = Vec::with_capacity(targets.len());
    for target in targets {
        if cancellation.load(Ordering::Acquire) {
            break;
        }
        tokio::select! {
            result = check_target(&client, target) => results.push(result),
            _ = async {
                while !cancellation.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            } => break,
        }
    }
    results
}

async fn check_target(client: &reqwest::Client, target: CheckTarget) -> CheckResult {
    let started = Instant::now();
    let checked_at_ms = crate::now();
    let response = client.get(&target.url).send().await;
    match response {
        Ok(response) => {
            let status = response.status();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| format!("{}: {}", name, value.to_str().unwrap_or("<binary>")))
                .collect::<Vec<_>>()
                .join("\n");
            match response.text().await {
                Ok(body) => {
                    let body = crate::clip(&body, MAX_RESPONSE_BYTES);
                    classify_response(
                        target,
                        checked_at_ms,
                        started.elapsed().as_millis() as u64,
                        Some(status.as_u16()),
                        headers,
                        body,
                    )
                }
                Err(error) if target.kind == CheckKind::HttpStatus && status.is_success() => {
                    CheckResult {
                        label: target.label,
                        url: target.url,
                        ok: true,
                        checked_at_ms,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        http_status: Some(status.as_u16()),
                        first_line: String::new(),
                        headers,
                        body: format!("[Could not read response body: {error}]"),
                        error: None,
                    }
                }
                Err(error) => CheckResult {
                    label: target.label,
                    url: target.url,
                    ok: false,
                    checked_at_ms,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    http_status: Some(status.as_u16()),
                    first_line: String::new(),
                    headers,
                    body: String::new(),
                    error: Some(format!("Could not read response body: {error}")),
                },
            }
        }
        Err(error) => CheckResult {
            label: target.label,
            url: target.url,
            ok: false,
            checked_at_ms,
            elapsed_ms: started.elapsed().as_millis() as u64,
            http_status: error.status().map(|status| status.as_u16()),
            first_line: String::new(),
            headers: String::new(),
            body: String::new(),
            error: Some(error.to_string()),
        },
    }
}

fn classify_response(
    target: CheckTarget,
    checked_at_ms: u64,
    elapsed_ms: u64,
    http_status: Option<u16>,
    headers: String,
    body: String,
) -> CheckResult {
    let first_line = body.lines().next().unwrap_or_default().trim().to_owned();
    let status_ok = http_status.is_some_and(|status| (200..300).contains(&status));
    let validation_error = match target.kind {
        CheckKind::Api if !status_ok => Some(format!(
            "API returned HTTP {}",
            http_status.map_or_else(|| "unknown".into(), |status| status.to_string())
        )),
        CheckKind::Api => match serde_json::from_str::<Value>(&body) {
            Ok(value) if value["ok"] == true => None,
            Ok(_) => Some("API response JSON did not contain ok=true".into()),
            Err(error) => Some(format!("API response was not valid JSON: {error}")),
        },
        CheckKind::HttpStatus if !status_ok => Some(format!(
            "Request returned HTTP {} (expected any 2xx status)",
            http_status.map_or_else(|| "unknown".into(), |status| status.to_string())
        )),
        CheckKind::HttpStatus => None,
        CheckKind::FirstLineOk if !status_ok => Some(format!(
            "Request returned HTTP {}",
            http_status.map_or_else(|| "unknown".into(), |status| status.to_string())
        )),
        CheckKind::FirstLineOk if first_line != "OK" => Some(format!(
            "Expected first response line to be OK, got {first_line:?}"
        )),
        CheckKind::FirstLineOk => None,
    };
    CheckResult {
        label: target.label,
        url: target.url,
        ok: validation_error.is_none(),
        checked_at_ms,
        elapsed_ms,
        http_status,
        first_line,
        headers,
        body,
        error: validation_error,
    }
}

fn format_timestamp(ms: u64) -> String {
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string())
        .unwrap_or_else(|| format!("{ms} ms since Unix epoch"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_ten_seconds_and_bounds_error_history() {
        let mut config = CheckerConfig::default();
        assert_eq!(config.interval_seconds, 10);
        config.interval_seconds = 0;
        config.errors = (0..510)
            .map(|index| ErrorRecord {
                at_ms: index,
                label: "test".into(),
                url: "http://127.0.0.1/test".into(),
                detail: index.to_string(),
            })
            .collect();
        config.normalize();
        assert_eq!(config.interval_seconds, 1);
        assert_eq!(config.errors.len(), MAX_ERROR_HISTORY);
        assert_eq!(config.errors.first().unwrap().at_ms, 10);
    }

    #[test]
    fn mcp_api_and_generic_http_validation_use_their_intended_rules() {
        let mcp = CheckTarget {
            label: "MCP URL".into(),
            url: "https://example.test/mcp".into(),
            kind: CheckKind::FirstLineOk,
            removable: true,
        };
        assert!(
            classify_response(
                mcp.clone(),
                1,
                2,
                Some(200),
                String::new(),
                "OK\ntools".into(),
            )
            .ok
        );
        assert!(
            !classify_response(mcp, 1, 2, Some(200), String::new(), "NOT OK\ntools".into(),).ok
        );

        let generic = CheckTarget {
            label: "HTTP".into(),
            url: "https://example.test/status".into(),
            kind: CheckKind::HttpStatus,
            removable: true,
        };
        for status in [200, 201, 204, 206, 299] {
            assert!(
                classify_response(
                    generic.clone(),
                    1,
                    2,
                    Some(status),
                    String::new(),
                    "anything".into(),
                )
                .ok,
                "status {status} should pass"
            );
        }
        for status in [199, 300, 404, 500] {
            assert!(
                !classify_response(
                    generic.clone(),
                    1,
                    2,
                    Some(status),
                    String::new(),
                    "anything".into(),
                )
                .ok,
                "status {status} should fail"
            );
        }
        assert_eq!(
            custom_check_kind("https://example.test/mcp"),
            CheckKind::FirstLineOk
        );
        assert_eq!(
            custom_check_kind("https://example.test/mcp/?x=1"),
            CheckKind::FirstLineOk
        );
        assert_eq!(
            custom_check_kind("https://example.test/"),
            CheckKind::HttpStatus
        );

        let api = CheckTarget {
            label: "API".into(),
            url: "http://127.0.0.1/health".into(),
            kind: CheckKind::Api,
            removable: false,
        };
        assert!(
            classify_response(
                api.clone(),
                1,
                2,
                Some(200),
                String::new(),
                r#"{"ok":true}"#.into(),
            )
            .ok
        );
        assert!(
            !classify_response(
                api,
                1,
                2,
                Some(200),
                String::new(),
                r#"{"ok":false}"#.into(),
            )
            .ok
        );
    }

    #[test]
    fn base_and_custom_urls_require_http_or_https() {
        assert!(normalize_base_url("http://127.0.0.1:3210/").is_ok());
        assert!(normalize_base_url("file:///tmp/x").is_err());
        assert!(valid_http_url("https://example.test/mcp"));
        assert!(!valid_http_url("not a url"));
    }

    #[test]
    fn custom_urls_persist_to_disk_and_reload_on_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "lessagent-service-check-persistence-{}-{}",
            std::process::id(),
            crate::now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let url = "https://example.test/persisted";

        let mut app = ServiceCheckerApp::new(dir.clone(), "http://127.0.0.1:3210".into());
        app.custom_url_input = url.into();
        app.add_custom_url();
        assert_eq!(app.config.custom_urls, vec![url.to_owned()]);
        assert!(app.input_error.is_none());
        assert!(config_path(&dir).is_file());

        let reopened = ServiceCheckerApp::new(dir.clone(), "http://127.0.0.1:3210".into());
        assert_eq!(reopened.config.custom_urls, vec![url.to_owned()]);
        assert!(reopened.targets().iter().any(|target| target.url == url));

        let mut reopened = reopened;
        reopened.remove_custom_url(url);
        assert!(reopened.config.custom_urls.is_empty());
        let reopened_again = ServiceCheckerApp::new(dir.clone(), "http://127.0.0.1:3210".into());
        assert!(reopened_again.config.custom_urls.is_empty());

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn fixture_app() -> (ServiceCheckerApp, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lessagent-checker-policy-{}", crate::id()));
        let app = ServiceCheckerApp::new(dir.clone(), "http://127.0.0.1:39999".into());
        (app, dir)
    }

    fn result(app: &ServiceCheckerApp, url: &str, ok: bool) -> CheckResult {
        let target = app.targets().into_iter().find(|t| t.url == url).unwrap();
        classify_response(
            target,
            crate::now(),
            1,
            Some(if ok { 200 } else { 503 }),
            String::new(),
            if url.ends_with("/health") {
                r#"{"ok":true}"#
            } else {
                "OK"
            }
            .into(),
        )
    }

    #[test]
    fn failures_sort_first_and_recovery_clears_alert_not_history() {
        let (mut app, dir) = fixture_app();
        app.custom_url_input = "https://example.test/failing".into();
        app.add_custom_url();
        let url = app.config.custom_urls[0].clone();
        let health = format!("{}/health", app.base_url);
        let mcp = format!("{}/mcp", app.base_url);
        app.apply_results(vec![
            result(&app, &health, true),
            result(&app, &mcp, false),
            result(&app, &url, false),
        ]);
        assert_eq!(app.active_failure_count(), 2);
        assert_eq!(app.sorted_targets()[0].url, mcp);
        assert_eq!(app.sorted_targets()[1].url, url);
        assert!(should_show_mini_alert(false, 2));
        assert!(!should_show_mini_alert(true, 2));
        app.apply_results(vec![result(&app, &mcp, true)]);
        assert_eq!(app.active_failure_count(), 1);
        assert_eq!(app.sorted_targets()[0].url, url);
        app.apply_results(vec![result(&app, &url, true)]);
        assert_eq!(app.active_failure_count(), 0);
        assert_eq!(app.config.errors.len(), 2);
        assert!(!should_show_mini_alert(false, app.active_failure_count()));
        let reopened = ServiceCheckerApp::new(dir.clone(), app.base_url.clone());
        assert_eq!(reopened.config.errors.len(), 2);
        assert_eq!(
            reopened.active_failure_count(),
            0,
            "history must not make a new popup"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pause_is_persistent_excludes_requests_and_resume_is_pending() {
        let (mut app, dir) = fixture_app();
        app.custom_url_input = "https://example.test/paused".into();
        app.add_custom_url();
        let url = app.config.custom_urls[0].clone();
        app.apply_results(vec![result(&app, &url, false)]);
        app.toggle_pause(&url);
        assert!(app.is_paused(&url));
        assert_eq!(app.active_failure_count(), 0);
        assert!(!app.active_targets().iter().any(|target| target.url == url));
        assert_eq!(app.sorted_targets().last().unwrap().url, url);
        let reopened = ServiceCheckerApp::new(dir.clone(), app.base_url.clone());
        assert!(reopened.is_paused(&url));
        app.apply_results(vec![result(&app, &url, false)]);
        assert_eq!(
            app.config.errors.len(),
            1,
            "late paused result must not add failures"
        );
        app.toggle_pause(&url);
        assert!(!app.is_paused(&url));
        assert!(app.result_for(&url).is_none());
        assert!(app.active_targets().iter().any(|target| target.url == url));
        app.toggle_pause(&url);
        app.remove_custom_url(&url);
        assert!(!load_config(&dir).paused_urls.contains(&url));
        assert!(app.result_for(&url).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pause_cancels_batch_and_discards_stale_results_after_resume() {
        let (mut app, dir) = fixture_app();
        let url = format!("{}/mcp", app.base_url);
        let old_revision = app.revision;
        let cancellation = Arc::new(AtomicBool::new(false));
        app.cancellation = Some(cancellation.clone());
        let (tx, rx) = mpsc::channel();
        app.receiver = Some(rx);
        app.checking = true;
        let stale = result(&app, &url, false);
        app.toggle_pause(&url);
        app.toggle_pause(&url);
        assert!(cancellation.load(Ordering::Acquire));
        tx.send((old_revision, vec![stale])).unwrap();
        app.receive_results();
        assert!(!app.checking);
        assert!(app.results.is_empty());
        assert!(app.config.errors.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn all_paused_skips_check_now_and_failed_save_keeps_old_state() {
        let (mut app, dir) = fixture_app();
        for target in app.targets() {
            app.toggle_pause(&target.url);
        }
        app.launch_checks();
        assert!(!app.checking);
        assert!(app.receiver.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, "not a directory").unwrap();
        let first = app.targets()[0].url.clone();
        app.toggle_pause(&first);
        assert!(
            app.is_paused(&first),
            "failed persistence must not pretend resume succeeded"
        );
        assert!(app.input_error.is_some());
        std::fs::remove_file(dir).unwrap();
    }
    #[test]
    fn disconnected_worker_reports_failure_and_does_not_stick() {
        let (mut app, dir) = fixture_app();
        let (sender, receiver) = mpsc::channel();
        app.receiver = Some(receiver);
        app.checking = true;
        drop(sender);
        app.receive_results();
        assert!(!app.checking);
        assert!(app.receiver.is_none());
        assert_eq!(app.active_failure_count(), app.targets().len());
        assert!(
            app.results
                .iter()
                .all(|r| r.error.as_deref().unwrap().contains("worker stopped"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
