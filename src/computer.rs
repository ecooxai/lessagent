use crate::{Result, err};
use serde_json::{Value, json};
use std::path::Path;

pub fn capabilities() -> Value {
    #[cfg(target_os = "macos")]
    {
        json!({"platform":"macos","backend":"CoreGraphics + AppKit + screencapture","background_control":true,"desktop_input":false,"pointer_idle_transparent_seconds":10,"pointer_idle_hide_seconds":30,"pointer_color":"#7DD4FF","note_background":"Use action windows, then window_id and pid for window-local input or screenshots. Process-directed events keep the system pointer independent; app support varies. Target must not be minimized.","note":"Enable Accessibility for the backend terminal, and Screen Recording for screenshots."})
    }
    #[cfg(not(target_os = "macos"))]
    {
        json!({"platform":"linux","backend":"xdotool + scrot (X11); grim screenshots (Wayland)","note":"Mouse and keyboard require an X11 session and xdotool. Native Wayland input injection is not supported."})
    }
}

/// Return the current primary display geometry when the platform exposes it.
/// `screen_width`/`screen_height` are screenshot pixels; logical dimensions are
/// the coordinate space accepted by the native input API.  On a Retina Mac
/// these values differ, so callers can map image coordinates before posting an
/// event.
pub fn screen_info() -> Option<Value> {
    #[cfg(target_os = "macos")]
    {
        mac::screen_info().map(|info| {
            json!({
                "screen_width": info.pixel_width,
                "screen_height": info.pixel_height,
                "logical_width": info.logical_width,
                "logical_height": info.logical_height,
                "origin_x": info.origin_x,
                "origin_y": info.origin_y,
                "scale_x": info.logical_width / info.pixel_width as f64,
                "scale_y": info.logical_height / info.pixel_height as f64
            })
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
/// Browser sizes are logical window points, never Retina screenshot pixels.
/// Unknown display geometry is not represented by a fabricated numeric limit.
pub fn browser_size_schema() -> Value {
    browser_size_schema_for(screen_info().as_ref())
}
fn browser_size_schema_for(display: Option<&Value>) -> Value {
    let mut properties = json!({});
    for (field, logical, minimum, default) in [
        ("width", "logical_width", 640, 1000),
        ("height", "logical_height", 480, 600),
    ] {
        let maximum = display.and_then(|v| v[logical].as_f64())
            .filter(|v| v.is_finite() && *v > 0.0).map(|v| v.floor() as u64);
        let mut schema = json!({"type":"integer", "minimum":minimum,
            "description":"Browser window size in logical points. Maximum is the current primary display, rechecked at launch; the actual window is fitted to its visible work area. See instruction.md. Not screenshot pixels."});
        if let Some(maximum) = maximum {
            schema["maximum"] = json!(maximum);
            if maximum >= minimum { schema["default"] = json!(default.min(maximum)); }
        } else {
            schema["default"] = json!(default);
        }
        properties[field] = schema;
    }
    properties
}
fn validate_browser_size(args: &Value, properties: &Value) -> Result<()> {
    for field in ["width", "height"] {
        if let Some(value) = args.get(field) {
            let minimum = properties[field]["minimum"].as_u64().unwrap();
            let maximum = properties[field]["maximum"].as_u64();
            if value.as_u64().is_none_or(|n| n < minimum || maximum.is_some_and(|m| n > m)) {
                return Err(err(format!("{field} must be an integer >= {minimum} and no larger than the current primary display in logical points (maximum: {}). Read instruction.md or refresh tools/list; no browser was opened.",
                    maximum.map(|v| v.to_string()).unwrap_or_else(|| "unavailable until launch".into()))));
            }
        }
    }
    Ok(())
}

pub fn action(args: &Value, capture: &Path) -> Result<Value> {
    validate_common_args(args)?;
    #[cfg(target_os = "macos")]
    if macos_background_route(args)? {
        return background_action(args, capture);
    }
    #[cfg(not(target_os = "macos"))]
    if args["action"] == "windows"
        || args.get("window_id").is_some()
        || args["mode"] == "background"
    {
        return Err(err("Background window control is supported on macOS only"));
    }
    let args = normalize_args(args)?;
    let action = args["action"]
        .as_str()
        .ok_or_else(|| err("Missing computer action"))?;
    if action == "screenshot" {
        std::fs::create_dir_all(
            capture
                .parent()
                .ok_or_else(|| err("Invalid screenshot path"))?,
        )?;
        #[cfg(target_os = "macos")]
        let status = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-t", "png"])
            .arg(capture)
            .status()?;
        #[cfg(not(target_os = "macos"))]
        let status = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            std::process::Command::new("grim").arg(capture).status()?
        } else {
            std::process::Command::new("scrot")
                .args(["--overwrite"])
                .arg(capture)
                .status()?
        };
        if !status.success() {
            return Err(err(
                "Screenshot failed; check screen recording permission and screenshot utility",
            ));
        }
        let mut result = json!({"path":capture,"mime":"image/png"});
        if let Some(info) = screen_info() {
            result["display"] = info;
        }
        let size = imagesize::size(capture)?;
        result["screen_width"] = json!(size.width);
        result["screen_height"] = json!(size.height);
        result["width"] = json!(size.width);
        result["height"] = json!(size.height);
        result["coordinate_space"] = json!("desktop");
        return Ok(result);
    }
    #[cfg(target_os = "macos")]
    {
        Err(err(
            "macOS system-pointer input is disabled; no input was sent",
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return Err(err(
                "Input control requires X11; Wayland supports screenshots only",
            ));
        }
        let mut c = std::process::Command::new("xdotool");
        match action {
            "move" | "click" | "drag" => {
                let x = number(&args, "x")?;
                let y = number(&args, "y")?;
                c.args(["mousemove", "--sync", &x.to_string(), &y.to_string()]);
                if action == "click" {
                    c.args(["click", if args["button"] == "right" { "3" } else { "1" }]);
                } else if action == "drag" {
                    let tx = number(&args, "to_x")?;
                    let ty = number(&args, "to_y")?;
                    c.args([
                        "mousedown",
                        "1",
                        "mousemove",
                        "--sync",
                        &tx.to_string(),
                        &ty.to_string(),
                        "mouseup",
                        "1",
                    ]);
                }
            }
            "type" => {
                c.args([
                    "type",
                    "--clearmodifiers",
                    "--",
                    args["text"].as_str().ok_or_else(|| err("Missing text"))?,
                ]);
            }
            "key" => {
                c.args([
                    "key",
                    "--clearmodifiers",
                    args["key"]
                        .as_str()
                        .filter(|s| !s.starts_with('-'))
                        .ok_or_else(|| err("Missing key"))?,
                ]);
            }
            "scroll" => {
                let delta = number(&args, "delta")?.clamp(-100, 100);
                if args.get("x").is_some() {
                    c.args([
                        "mousemove",
                        "--sync",
                        &number(&args, "x")?.to_string(),
                        &number(&args, "y")?.to_string(),
                    ]);
                }
                if delta == 0 {
                    return Ok(json!({"ok":true,"action":action,"meaningful":false}));
                }
                c.args([
                    "click",
                    "--repeat",
                    &delta.unsigned_abs().to_string(),
                    if delta < 0 { "4" } else { "5" },
                ]);
            }
            _ => return Err(err("Unknown computer action")),
        }
        if !c.status()?.success() {
            return Err(err("xdotool failed"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    Ok(json!({"ok":true,"action":action,"meaningful":true}))
}

/// Shared by agent, HTTP and MCP: incomplete window requests must never
/// become global mouse/keyboard input. macOS has no desktop input backend.
#[cfg(any(target_os = "macos", test))]
fn macos_background_route(args: &Value) -> Result<bool> {
    validate_common_args(args)?;
    if args["mode"] == "desktop" {
        return Err(err(
            "macOS permits background window control only; desktop/system-pointer input is disabled and never used as a fallback",
        ));
    }
    if args["action"] == "windows" || args["action"] == "browser_open" {
        return Ok(true);
    }
    if args.get("window_id").is_some() || args.get("pid").is_some() || args["mode"] == "background"
    {
        return Ok(true);
    }
    // A first untargeted screenshot is useful for desktop discovery.
    if args["action"] == "screenshot" {
        return Ok(false);
    }
    Err(err(
        "No background window selected. Call windows, then pass window_id and pid. macOS system-pointer input is disabled; no input was sent",
    ))
}

/// Schemas are hints, not enforcement. Validate before creating native events.
fn validate_common_args(args: &Value) -> Result<()> {
    if !args.is_object() {
        return Err(err("Computer arguments must be an object"));
    }
    if !matches!(
        args["action"].as_str(),
        Some(
            "windows"
                | "screenshot"
                | "browser_open"
                | "move"
                | "click"
                | "drag"
                | "type"
                | "key"
                | "scroll"
        )
    ) {
        return Err(err("Unknown or missing computer action"));
    }
    if args["action"] == "browser_open" {
        let url = args["url"]
            .as_str()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .ok_or_else(|| err("browser_open requires an HTTP(S) URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(err(
                "browser_open requires an HTTP(S) URL without embedded credentials",
            ));
        }
        validate_browser_size(args, &browser_size_schema())?;
        if [
            "window_id",
            "pid",
            "x",
            "y",
            "path",
            "button",
            "text",
            "key",
        ]
        .iter()
        .any(|key| args.get(*key).is_some())
        {
            return Err(err(
                "browser_open creates a new isolated window; do not supply an existing target or input fields",
            ));
        }
    }
    if let Some(mode) = args.get("mode")
        && !matches!(mode.as_str(), Some("background" | "desktop"))
    {
        return Err(err("mode must be background or desktop"));
    }
    if args["mode"] == "desktop" && (args.get("window_id").is_some() || args.get("pid").is_some()) {
        return Err(err(
            "A window target requires background mode; remove window_id and pid for desktop control",
        ));
    }
    if let Some(button) = args.get("button")
        && !matches!(button.as_str(), Some("left" | "right"))
    {
        return Err(err("button must be left or right"));
    }
    if let Some(show) = args.get("show_pointer")
        && !show.is_boolean()
    {
        return Err(err("show_pointer must be a boolean"));
    }
    if args.get("screen_width").is_some() || args.get("screen_height").is_some() {
        for field in ["screen_width", "screen_height"] {
            if number(args, field)? <= 0 {
                return Err(err("Screenshot dimensions must both be positive integers"));
            }
        }
    }
    if args.get("duration").is_some() && args.get("duration_ms").is_some() {
        return Err(err("Use duration or duration_ms, not both"));
    }
    for (field, low, high) in [("duration", 0.05, 5.0), ("duration_ms", 50.0, 5000.0)] {
        if let Some(v) = args.get(field)
            && v.as_f64()
                .filter(|n| n.is_finite() && (low..=high).contains(n))
                .is_none()
        {
            return Err(err(format!("{field} must be between {low} and {high}")));
        }
    }
    if args.get("path").is_some() && args["action"] != "drag" {
        return Err(err("path is only supported for drag"));
    }
    if matches!(args["action"].as_str(), Some("move" | "click")) {
        number(args, "x")?;
        number(args, "y")?;
    }
    if args["action"] == "type"
        && args["text"]
            .as_str()
            .filter(|s| s.encode_utf16().count() <= 16384)
            .is_none()
    {
        return Err(err("Missing text or text exceeds 16384 UTF-16 units"));
    }
    if args["action"] == "key" && args["key"].as_str().filter(|s| !s.is_empty()).is_none() {
        return Err(err("Missing key"));
    }
    Ok(())
}

/// Validate routing and the entire stroke before starting a native helper.
#[cfg(any(target_os = "macos", test))]
fn normalize_background_args(args: &Value) -> Result<Value> {
    validate_common_args(args)?;
    if args["action"] == "windows" || args["action"] == "browser_open" {
        return Ok(args.clone());
    }
    if args["mode"] == "desktop" {
        return Err(err("A window target requires background mode"));
    }
    for key in ["window_id", "pid"] {
        if args[key]
            .as_u64()
            .filter(|n| *n > 0 && *n <= i32::MAX as u64)
            .is_none()
        {
            return Err(err(format!(
                "Background input requires a positive integer {key}; call windows first"
            )));
        }
    }
    if args["action"] != "drag" || args.get("path").is_none() {
        return normalize_args(args);
    }
    let path = args["path"]
        .as_array()
        .filter(|p| (2..=512).contains(&p.len()))
        .ok_or_else(|| err("path needs 2 to 512 [x,y] points"))?;
    for point in path {
        let pair = point
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(|| err("path points must be [x,y]"))?;
        if pair.iter().any(|n| {
            n.as_f64()
                .filter(|v| v.is_finite() && *v >= 0. && *v <= i32::MAX as f64)
                .is_none()
        }) {
            return Err(err("Invalid path coordinate"));
        }
    }
    let mut value = args.clone();
    value["x"] = path[0][0].clone();
    value["y"] = path[0][1].clone();
    Ok(value)
}

/// One serialized, persistent AppKit helper owns the virtual pointer and its
/// idle timers. A broken connection fails the current request: never replay
/// input or substitute a desktop event. The next explicit call may start a new
/// isolated helper. Closing the parent's pipe terminates the helper on shutdown.
#[cfg(target_os = "macos")]
fn background_action(args: &Value, capture: &Path) -> Result<Value> {
    let mut args = normalize_background_args(args)?;
    if args["action"] == "screenshot" {
        std::fs::create_dir_all(
            capture
                .parent()
                .ok_or_else(|| err("Invalid screenshot path"))?,
        )?;
        args["capture_path"] = json!(capture);
    }
    native_request(&args)
}

/// Resource discovery is read-only and does not require computer control enabled.
#[cfg(target_os = "macos")]
pub(crate) fn host_info() -> Result<Value> {
    native_request(&json!({"action":"system_info"}))
}
#[cfg(target_os = "macos")]
fn native_request(args: &Value) -> Result<Value> {
    static HELPER: std::sync::Mutex<Option<NativeHelper>> = std::sync::Mutex::new(None);
    let mut helper = HELPER
        .lock()
        .map_err(|_| err("Computer control lock poisoned"))?;
    if let Some(h) = helper.as_mut()
        && h.child.try_wait()?.is_some()
    {
        *helper = None;
    }
    if helper.is_none() {
        *helper = Some(NativeHelper::start()?);
    }
    let result = match helper.as_mut().unwrap().exchange(args) {
        Ok(result) => result,
        Err(error) => {
            *helper = None;
            return Err(err(format!(
                "macOS helper connection failed; input was not retried and no desktop fallback was used. Check the target before retrying: {error}"
            )));
        }
    };
    if let Some(error) = result.get("error") {
        return Err(err(error
            .as_str()
            .unwrap_or("macOS background input failed")));
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
struct NativeHelper {
    child: std::process::Child,
    directory: std::path::PathBuf,
    input: std::process::ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
}
#[cfg(target_os = "macos")]
impl NativeHelper {
    fn start() -> Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        use std::process::{Command, Stdio};
        let dir = std::env::temp_dir().join(format!("lessagent-computer-{}", crate::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        struct Cleanup(Option<std::path::PathBuf>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(path) = &self.0 {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
        // macOS code-signing can kill a newly launched executable if unlinked
        // before startup. Keep it for the helper's lifetime, in a private dir.
        let mut cleanup = Cleanup(Some(dir.clone()));
        let executable = dir.join("lessagent-computer");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&executable)?;
        file.write_all(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/lessagent-computer"
        )))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o500))?;
        drop(file);
        let mut child = Command::new(executable)
            .args(["--server", "--cleanup"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| err("Cannot open helper input"))?;
        let output = std::io::BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| err("Cannot open helper output"))?,
        );
        cleanup.0.take();
        Ok(Self {
            child,
            directory: dir,
            input,
            output,
        })
    }
    fn exchange(&mut self, args: &Value) -> Result<Value> {
        use std::io::{BufRead, Write};
        serde_json::to_writer(&mut self.input, args)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        let mut line = String::new();
        if self.output.read_line(&mut line)? == 0 {
            return Err(err("Native helper exited without a response"));
        }
        Ok(serde_json::from_str(&line)?)
    }
}
#[cfg(target_os = "macos")]
impl Drop for NativeHelper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Normalize the small amount of shorthand that models commonly use for a
/// drag, then validate its endpoints before any native event is posted. This
/// is deliberately done on the cloned argument value so a malformed request
/// can never move the pointer as a side effect of failing validation.
fn normalize_args(args: &Value) -> Result<Value> {
    validate_common_args(args)?;
    let mut args = args.clone();
    if args["action"] == "scroll" {
        if args.get("distance").is_some() {
            if args.get("delta").is_some() {
                return Err(err("Use distance or delta, not both"));
            }
            let distance = number(&args, "distance")?;
            if !(-100..=100).contains(&distance) {
                return Err(err("distance must be between -100 and 100"));
            }
            args["delta"] = json!(-distance);
        }
        number(&args, "delta")?;
        if args.get("x").is_some() || args.get("y").is_some() {
            number(&args, "x")?;
            number(&args, "y")?;
        }
    }
    if args["action"].as_str() != Some("drag") {
        return Ok(args);
    }
    let object = args
        .as_object_mut()
        .ok_or_else(|| err("Computer arguments must be an object"))?;

    let from = object
        .get("from")
        .or_else(|| object.get("start"))
        .and_then(pair)
        .or_else(|| {
            let x = object.get("start_x").cloned();
            let y = object.get("start_y").cloned();
            x.zip(y)
        });
    if let Some((x, y)) = from {
        object.entry("x").or_insert(x);
        object.entry("y").or_insert(y);
    }
    let to = object
        .get("to")
        .or_else(|| object.get("end"))
        .and_then(pair)
        .or_else(|| {
            let x = object
                .get("end_x")
                .or_else(|| object.get("target_x"))
                .cloned();
            let y = object
                .get("end_y")
                .or_else(|| object.get("target_y"))
                .cloned();
            x.zip(y)
        });
    if let Some((x, y)) = to {
        object.entry("to_x").or_insert(x);
        object.entry("to_y").or_insert(y);
    }

    // A horizontal or vertical shorthand is unambiguous when one destination
    // coordinate is present: keep the omitted axis at the starting position.
    // This accepts the form emitted by older Light prompts while still
    // requiring a real start and at least one destination coordinate.
    if !object.contains_key("to_x")
        && object.contains_key("to_y")
        && let Some(x) = object.get("x").cloned()
    {
        object.insert("to_x".into(), x);
    }
    if !object.contains_key("to_y")
        && object.contains_key("to_x")
        && let Some(y) = object.get("y").cloned()
    {
        object.insert("to_y".into(), y);
    }

    // Validate every coordinate before the platform backend is entered. The
    // backend's number() check also rejects strings, booleans, and fractions.
    for key in ["x", "y", "to_x", "to_y"] {
        let n = object
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| err(format!("Missing integer {key} for drag")))?;
        i32::try_from(n).map_err(|_| err("Drag coordinate out of range"))?;
    }
    Ok(args)
}

fn pair(value: &Value) -> Option<(Value, Value)> {
    if let Some(values) = value.as_array() {
        return values.first().cloned().zip(values.get(1).cloned());
    }
    let object = value.as_object()?;
    let x = object.get("x").or_else(|| object.get("left")).cloned();
    let y = object.get("y").or_else(|| object.get("top")).cloned();
    x.zip(y)
}
fn number(v: &Value, key: &str) -> Result<i32> {
    let n = v[key]
        .as_i64()
        .ok_or_else(|| err(format!("Missing integer {key}")))?;
    i32::try_from(n).map_err(|_| err("Coordinate out of range"))
}
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    #[derive(Clone, Copy)]
    pub(super) struct ScreenInfo {
        pub pixel_width: usize,
        pub pixel_height: usize,
        pub logical_width: f64,
        pub logical_height: f64,
        pub origin_x: f64,
        pub origin_y: f64,
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(object: *const c_void);
        fn CGMainDisplayID() -> u32;
        fn CGDisplayPixelsWide(display: u32) -> usize;
        fn CGDisplayPixelsHigh(display: u32) -> usize;
        fn CGDisplayCopyDisplayMode(display: u32) -> *const c_void;
        fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
        fn CGDisplayModeGetPixelHeight(mode: *const c_void) -> usize;
        fn CGDisplayBounds(display: u32) -> Rect;
    }
    pub(super) fn screen_info() -> Option<ScreenInfo> {
        // These CoreGraphics calls are read-only and are available on every
        // supported macOS release. A zero-sized display is treated as absent.
        let display = unsafe { CGMainDisplayID() };
        // CGDisplayPixelsWide/High report the logical mode on Retina Macs.
        // CGDisplayModeGetPixel* gives the backing-pixel dimensions used by
        // screencapture, which is the coordinate space attached to screenshots.
        let mode = unsafe { CGDisplayCopyDisplayMode(display) };
        let mode_pixel_width = if !mode.is_null() {
            unsafe { CGDisplayModeGetPixelWidth(mode) }
        } else {
            0
        };
        let mode_pixel_height = if !mode.is_null() {
            unsafe { CGDisplayModeGetPixelHeight(mode) }
        } else {
            0
        };
        if !mode.is_null() {
            unsafe { CFRelease(mode) };
        }
        let pixel_width = if mode_pixel_width > 0 {
            mode_pixel_width
        } else {
            unsafe { CGDisplayPixelsWide(display) }
        };
        let pixel_height = if mode_pixel_height > 0 {
            mode_pixel_height
        } else {
            unsafe { CGDisplayPixelsHigh(display) }
        };
        let bounds = unsafe { CGDisplayBounds(display) };
        if pixel_width == 0
            || pixel_height == 0
            || !bounds.size.width.is_finite()
            || !bounds.size.height.is_finite()
            || bounds.size.width <= 0.
            || bounds.size.height <= 0.
        {
            return None;
        }
        Some(ScreenInfo {
            pixel_width,
            pixel_height,
            logical_width: bounds.size.width,
            logical_height: bounds.size.height,
            origin_x: bounds.origin.x,
            origin_y: bounds.origin.y,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_args;
    use serde_json::json;

    #[test]
    fn browser_open_validates_before_launching_any_process() {
        for value in [
            json!({"action":"browser_open"}),
            json!({"action":"browser_open","url":"file:///etc/passwd"}),
            json!({"action":"browser_open","url":"javascript:alert(1)"}),
            json!({"action":"browser_open","url":"https://user:secret@example.com"}),
            json!({"action":"browser_open","url":"http://127.0.0.1:4173","width":true}),
            json!({"action":"browser_open","url":"https://example.com","height":600.5}),
            json!({"action":"browser_open","url":"https://example.com","width":0}),
            json!({"action":"browser_open","url":"https://example.com","window_id":4,"pid":5}),
        ] {
            assert!(super::validate_common_args(&value).is_err(), "{value}");
        }
        let value = json!({"action":"browser_open","url":"http://127.0.0.1:4173/","width":1000,"height":750});
        assert!(super::validate_common_args(&value).is_ok());
        assert!(super::macos_background_route(&value).unwrap());
        assert!(super::normalize_background_args(&value).is_ok());
    }

    #[test]
    fn background_routing_never_falls_back_to_system_pointer() {
        use super::macos_background_route as route;
        for value in [
            json!({"action":"click","x":10,"y":20}),
            json!({"action":"move","x":10,"y":20}),
            json!({"action":"drag","x":10,"y":20,"to_x":30,"to_y":40}),
            json!({"action":"key","key":"a"}),
            json!({"action":"type","text":"hello"}),
            json!({"action":"scroll","distance":-1}),
            json!({"action":"click","mode":"bckground","x":10,"y":20}),
            json!({"action":"click","mode":"desktop","pid":9,"x":10,"y":20}),
        ] {
            assert!(route(&value).is_err(), "{value}");
        }
        assert!(!route(&json!({"action":"screenshot"})).unwrap());
        assert!(route(&json!({"action":"windows"})).unwrap());
        assert!(route(&json!({"action":"click","mode":"desktop","x":10,"y":20})).is_err());
        // A PID-only request must be routed to validation, never desktop.
        assert!(route(&json!({"action":"click","pid":9,"x":10,"y":20})).unwrap());
        assert!(
            super::normalize_background_args(&json!({"action":"click","pid":9,"x":10,"y":20}))
                .is_err()
        );
    }

    #[test]
    fn macos_rejects_explicit_desktop_for_every_action() {
        for action in [
            "windows",
            "screenshot",
            "move",
            "click",
            "drag",
            "scroll",
            "key",
            "type",
        ] {
            let value = json!({"action":action,"mode":"desktop","x":10,"y":20,"to_x":30,"to_y":40,"distance":1,"key":"a","text":"test"});
            assert!(super::macos_background_route(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn macos_native_backend_contains_no_global_input_api() {
        let source = include_str!("../native/computer.swift");
        for forbidden in [
            "CGEventPost(",
            ".post(tap:",
            "CGWarpMouseCursorPosition",
            "CGAssociateMouseAndMouseCursorPosition",
            "AXUIElementSetAttributeValue",
        ] {
            assert!(
                !source.contains(forbidden),
                "Forbidden global input API: {forbidden}"
            );
        }
        assert!(source.contains(".postToPid(pid)"));
    }

    #[test]
    fn common_validation_rejects_malformed_fields_before_native_events() {
        let base = json!({"action":"click","window_id":5,"pid":9,"x":20,"y":30});
        for patch in [
            json!({"button":"middle"}),
            json!({"button":false}),
            json!({"mode":"auto"}),
            json!({"show_pointer":"false"}),
            json!({"screen_width":1000}),
            json!({"screen_height":750}),
            json!({"screen_width":0,"screen_height":750}),
            json!({"screen_width":1000.5,"screen_height":750}),
            json!({"duration":-1}),
            json!({"duration":true}),
            json!({"duration":6}),
            json!({"duration_ms":10}),
            json!({"duration":1,"duration_ms":1000}),
            json!({"x":2147483648_i64}),
            json!({"x":"20"}),
            json!({"path":[[20,30],[40,50]]}),
        ] {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(super::normalize_background_args(&value).is_err(), "{value}");
        }
        for value in [
            json!(null),
            json!([]),
            json!({}),
            json!({"action":"unknown"}),
            json!({"action":"key","key":""}),
            json!({"action":"type","text":9}),
        ] {
            assert!(super::validate_common_args(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn path_drag_durations_and_dimensions_do_not_bypass_validation() {
        let base = json!({"action":"drag","window_id":5,"pid":9,"path":[[20,30],[40,50]]});
        for patch in [
            json!({"duration":-1}),
            json!({"duration_ms":true}),
            json!({"duration":0.1,"duration_ms":100}),
            json!({"screen_width":0,"screen_height":750}),
        ] {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(super::normalize_background_args(&value).is_err(), "{value}");
        }
        let mut value = base;
        value["screen_width"] = json!(2000);
        value["screen_height"] = json!(1500);
        value["duration_ms"] = json!(50);
        assert!(super::normalize_background_args(&value).is_ok());
    }

    #[test]
    fn drag_aliases_validate_full_integer_range() {
        for value in [
            json!({"action":"drag","start":[10,20],"end":[30,40]}),
            json!({"action":"drag","start_x":10,"start_y":20,"end_x":30,"end_y":40}),
            json!({"action":"drag","start_x":10,"start_y":20,"target_x":30,"target_y":40}),
        ] {
            let v = normalize_args(&value).unwrap();
            assert_eq!(
                (
                    v["x"].as_i64(),
                    v["y"].as_i64(),
                    v["to_x"].as_i64(),
                    v["to_y"].as_i64()
                ),
                (Some(10), Some(20), Some(30), Some(40))
            );
        }
        assert!(
            normalize_args(&json!({"action":"drag","x":10,"y":20,"to_x":2147483648_i64,"to_y":40}))
                .is_err()
        );
    }

    #[test]
    fn scroll_distance_and_coordinates_validate_before_input() {
        for distance in [-100, -10, 0, 10, 100] {
            let v = normalize_args(&json!({"action":"scroll","distance":distance,"x":20,"y":30}))
                .unwrap();
            assert_eq!(v["delta"], -distance);
        }
        for v in [
            json!({"action":"scroll","distance":101}),
            json!({"action":"scroll","distance":10,"delta":10}),
            json!({"action":"scroll","distance":10,"x":1}),
            json!({"action":"scroll","distance":1.5}),
            json!({"action":"scroll"}),
        ] {
            assert!(normalize_args(&v).is_err());
        }
    }

    #[test]
    fn background_requires_explicit_window_and_owner() {
        for value in [
            json!({"action":"click","mode":"background","x":1,"y":2}),
            json!({"action":"click","window_id":5,"pid":false}),
            json!({"action":"click","window_id":5,"pid":9,"mode":"desktop"}),
        ] {
            assert!(super::normalize_background_args(&value).is_err());
        }
        assert!(super::normalize_background_args(&json!({"action":"windows"})).is_ok());
    }

    #[test]
    fn background_validates_whole_path_before_input() {
        let base = json!({"action":"drag","window_id":5,"pid":9,"path":[[10,20],[30.5,40]]});
        let valid = super::normalize_background_args(&base).unwrap();
        assert_eq!(valid["x"], 10);
        assert_eq!(valid["path"][1][0], 30.5);
        for path in [
            json!([]),
            json!([[1, 2]]),
            json!([[1, 2], [-1, 3]]),
            json!([[1, 2], [3]]),
            json!([[1, 2], ["3", 4]]),
        ] {
            let mut bad = base.clone();
            bad["path"] = path;
            assert!(super::normalize_background_args(&bad).is_err());
        }
    }

    #[test]
    fn drag_normalization_keeps_omitted_axis_at_start() {
        let value = normalize_args(&json!({
            "action": "drag",
            "x": 100,
            "y": 200,
            "to_x": 340,
            "duration": 0.25
        }))
        .unwrap();
        assert_eq!(value["x"], 100);
        assert_eq!(value["y"], 200);
        assert_eq!(value["to_x"], 340);
        assert_eq!(value["to_y"], 200);
        assert_eq!(value["duration"], 0.25);
    }

    #[test]
    fn drag_normalization_accepts_from_and_to_pairs() {
        let value = normalize_args(&json!({
            "action": "drag",
            "from": [10, 20],
            "to": {"left": 30, "top": 40}
        }))
        .unwrap();
        assert_eq!(value["x"], 10);
        assert_eq!(value["y"], 20);
        assert_eq!(value["to_x"], 30);
        assert_eq!(value["to_y"], 40);
    }

    #[test]
    fn invalid_drag_is_rejected_before_backend_input() {
        let error = normalize_args(&json!({
            "action": "drag",
            "x": 10,
            "y": 20
        }))
        .expect_err("a drag without a destination must fail validation");
        assert!(error.to_string().contains("to_x"));
    }
}
