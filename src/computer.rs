use crate::{Result, err};
use serde_json::{Value, json};
use std::path::Path;

pub fn capabilities() -> Value {
    #[cfg(target_os = "macos")]
    {
        json!({"platform":"macos","backend":"CoreGraphics + screencapture","note":"Enable Accessibility for the backend terminal, and Screen Recording for screenshots."})
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
pub fn action(args: &Value, capture: &Path) -> Result<Value> {
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
        if let Ok(size) = imagesize::size(capture) {
            result["screen_width"] = json!(size.width);
            result["screen_height"] = json!(size.height);
        }
        if let Some(info) = screen_info() {
            if let Some(object) = info.as_object() {
                for (key, value) in object {
                    result[key] = value.clone();
                }
            }
        }
        return Ok(result);
    }
    #[cfg(target_os = "macos")]
    {
        mac::action(&args)?;
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
                let x = number(args, "x")?;
                let y = number(args, "y")?;
                c.args(["mousemove", "--sync", &x.to_string(), &y.to_string()]);
                if action == "click" {
                    c.args(["click", if args["button"] == "right" { "3" } else { "1" }]);
                } else if action == "drag" {
                    let tx = number(args, "to_x")?;
                    let ty = number(args, "to_y")?;
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
                let delta = number(args, "delta")?.clamp(-100, 100);
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
    Ok(json!({"ok":true,"action":action,"meaningful":true}))
}

/// Normalize the small amount of shorthand that models commonly use for a
/// drag, then validate its endpoints before any native event is posted. This
/// is deliberately done on the cloned argument value so a malformed request
/// can never move the pointer as a side effect of failing validation.
fn normalize_args(args: &Value) -> Result<Value> {
    let mut args = args.clone();
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
        if object.get(key).and_then(Value::as_i64).is_none() {
            return Err(err(format!("Missing integer {key} for drag")));
        }
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
    use super::*;
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
    type Event = *mut c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn CGEventCreateMouseEvent(
            source: *const c_void,
            kind: u32,
            point: Point,
            button: u32,
        ) -> Event;
        fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> Event;
        fn CGEventKeyboardSetUnicodeString(event: Event, len: usize, text: *const u16);
        fn CGEventSetFlags(event: Event, flags: u64);
        fn CGEventCreateScrollWheelEvent(
            source: *const c_void,
            units: u32,
            wheels: u32,
            ...
        ) -> Event;
        fn CGEventPost(tap: u32, event: Event);
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
        let mode_pixel_width = (!mode.is_null())
            .then(|| unsafe { CGDisplayModeGetPixelWidth(mode) })
            .unwrap_or(0);
        let mode_pixel_height = (!mode.is_null())
            .then(|| unsafe { CGDisplayModeGetPixelHeight(mode) })
            .unwrap_or(0);
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
    fn coordinate(v: &Value, x_key: &str, y_key: &str) -> Result<Point> {
        let x = number(v, x_key)? as f64;
        let y = number(v, y_key)? as f64;
        // Light mode and native callers can provide the screenshot dimensions.
        // Convert those pixels into Quartz points so Retina displays receive
        // the same location the model saw in the image. Without dimensions,
        // preserve the historical logical-point behavior for compatibility.
        let Some(info) = screen_info() else {
            return Ok(Point { x, y });
        };
        let source_width = v["screen_width"].as_f64().filter(|n| *n > 0.);
        let source_height = v["screen_height"].as_f64().filter(|n| *n > 0.);
        match (source_width, source_height) {
            (Some(width), Some(height)) => Ok(Point {
                x: info.origin_x + x * info.logical_width / width,
                y: info.origin_y + y * info.logical_height / height,
            }),
            _ => Ok(Point { x, y }),
        }
    }
    unsafe fn post(e: Event) -> Result<()> {
        if e.is_null() {
            return Err(err("Cannot create input event"));
        }
        unsafe {
            CGEventPost(0, e);
            CFRelease(e);
        }
        Ok(())
    }
    pub fn action(v: &Value) -> Result<()> {
        unsafe {
            if !AXIsProcessTrusted() {
                return Err(err(
                    "Enable Accessibility permission for the process running lessagent",
                ));
            }
            match v["action"].as_str().unwrap_or("") {
                "move" => {
                    let p = coordinate(v, "x", "y")?;
                    post(CGEventCreateMouseEvent(std::ptr::null(), 5, p, 0))?;
                }
                "click" => {
                    let p = coordinate(v, "x", "y")?;
                    let right = v["button"] == "right";
                    let button = if right { 1 } else { 0 };
                    post(CGEventCreateMouseEvent(std::ptr::null(), 5, p, 0))?;
                    post(CGEventCreateMouseEvent(
                        std::ptr::null(),
                        if right { 3 } else { 1 },
                        p,
                        button,
                    ))?;
                    post(CGEventCreateMouseEvent(
                        std::ptr::null(),
                        if right { 4 } else { 2 },
                        p,
                        button,
                    ))?;
                }
                "drag" => {
                    // Resolve both points before posting the initial move. A
                    // missing endpoint must be a clean error, never a pointer
                    // move that looks like a partially executed drag.
                    let start = coordinate(v, "x", "y")?;
                    let end = coordinate(v, "to_x", "to_y")?;
                    let right = v["button"] == "right";
                    let button = if right { 1 } else { 0 };
                    let down_kind = if right { 3 } else { 1 };
                    let up_kind = if right { 4 } else { 2 };
                    let dragged_kind = if right { 7 } else { 6 };
                    let distance = (end.x - start.x).hypot(end.y - start.y);
                    let steps = ((distance / 12.0).ceil() as usize).clamp(12, 120);
                    let seconds = v["duration"]
                        .as_f64()
                        .filter(|value| value.is_finite() && *value > 0.)
                        .or_else(|| {
                            v["duration_ms"]
                                .as_f64()
                                .filter(|value| value.is_finite() && *value > 0.)
                                .map(|value| value / 1000.)
                        })
                        .unwrap_or(0.15)
                        .clamp(0.05, 5.0);
                    let pause = std::time::Duration::from_secs_f64(seconds / steps as f64);

                    post(CGEventCreateMouseEvent(std::ptr::null(), 5, start, 0))?;
                    post(CGEventCreateMouseEvent(
                        std::ptr::null(),
                        down_kind,
                        start,
                        button,
                    ))?;
                    // Give the target application a frame to observe the
                    // button-down before the first dragged event.
                    std::thread::sleep(std::time::Duration::from_millis(8));
                    for i in 1..=steps {
                        let f = i as f64 / steps as f64;
                        post(CGEventCreateMouseEvent(
                            std::ptr::null(),
                            dragged_kind,
                            Point {
                                x: start.x + (end.x - start.x) * f,
                                y: start.y + (end.y - start.y) * f,
                            },
                            button,
                        ))?;
                        std::thread::sleep(pause);
                    }
                    post(CGEventCreateMouseEvent(
                        std::ptr::null(),
                        up_kind,
                        end,
                        button,
                    ))?;
                }
                "type" => {
                    let text: Vec<u16> = v["text"]
                        .as_str()
                        .ok_or_else(|| err("Missing text"))?
                        .encode_utf16()
                        .collect();
                    for down in [true, false] {
                        let e = CGEventCreateKeyboardEvent(std::ptr::null(), 0, down);
                        if e.is_null() {
                            return Err(err("Cannot create keyboard event"));
                        }
                        CGEventKeyboardSetUnicodeString(e, text.len(), text.as_ptr());
                        post(e)?;
                    }
                }
                "key" => {
                    let key = v["key"]
                        .as_str()
                        .ok_or_else(|| err("Missing key"))?
                        .to_ascii_lowercase();
                    let mut parts: Vec<_> = key.split('+').collect();
                    let key = parts.pop().unwrap_or("");
                    let mut flags = 0;
                    for m in parts {
                        flags |= match m {
                            "cmd" | "super" | "meta" => 1 << 20,
                            "ctrl" | "control" => 1 << 18,
                            "alt" | "option" => 1 << 19,
                            "shift" => 1 << 17,
                            _ => return Err(err("Unknown key modifier")),
                        };
                    }
                    let code = match key {
                        "a" => 0,
                        "s" => 1,
                        "d" => 2,
                        "f" => 3,
                        "h" => 4,
                        "g" => 5,
                        "z" => 6,
                        "x" => 7,
                        "c" => 8,
                        "v" => 9,
                        "b" => 11,
                        "q" => 12,
                        "w" => 13,
                        "e" => 14,
                        "r" => 15,
                        "y" => 16,
                        "t" => 17,
                        "1" => 18,
                        "2" => 19,
                        "3" => 20,
                        "4" => 21,
                        "6" => 22,
                        "5" => 23,
                        "9" => 25,
                        "7" => 26,
                        "8" => 28,
                        "0" => 29,
                        "o" => 31,
                        "u" => 32,
                        "i" => 34,
                        "p" => 35,
                        "enter" | "return" => 36,
                        "l" => 37,
                        "j" => 38,
                        "k" => 40,
                        "n" => 45,
                        "m" => 46,
                        "tab" => 48,
                        "space" => 49,
                        "backspace" => 51,
                        "escape" | "esc" => 53,
                        "delete" => 117,
                        "home" => 115,
                        "end" => 119,
                        "pageup" => 116,
                        "pagedown" => 121,
                        "left" => 123,
                        "right" => 124,
                        "down" => 125,
                        "up" => 126,
                        _ => return Err(err("Unsupported key; use type for text")),
                    };
                    for down in [true, false] {
                        let e = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
                        if e.is_null() {
                            return Err(err("Cannot create keyboard event"));
                        }
                        // Modifier flags describe the key state while the
                        // key is pressed. Clear them on key-up so a
                        // cmd/ctrl/option shortcut cannot leak into the next
                        // Unicode typing event.
                        CGEventSetFlags(e, if down { flags } else { 0 });
                        post(e)?;
                    }
                }
                "scroll" => {
                    post(CGEventCreateScrollWheelEvent(
                        std::ptr::null(),
                        1,
                        1,
                        -number(v, "delta")?.clamp(-100, 100),
                    ))?;
                }
                _ => return Err(err("Unknown computer action")),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_args;
    use serde_json::json;

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
