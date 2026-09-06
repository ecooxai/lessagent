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
pub fn action(args: &Value, capture: &Path) -> Result<Value> {
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
        return Ok(json!({"path":capture,"mime":"image/png"}));
    }
    #[cfg(target_os = "macos")]
    {
        mac::action(args)?;
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
    Ok(json!({"ok":true,"action":action}))
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
                "move" | "click" | "drag" => {
                    let p = Point {
                        x: number(v, "x")? as f64,
                        y: number(v, "y")? as f64,
                    };
                    post(CGEventCreateMouseEvent(std::ptr::null(), 5, p, 0))?;
                    if v["action"] == "click" {
                        let right = v["button"] == "right";
                        post(CGEventCreateMouseEvent(
                            std::ptr::null(),
                            if right { 3 } else { 1 },
                            p,
                            if right { 1 } else { 0 },
                        ))?;
                        post(CGEventCreateMouseEvent(
                            std::ptr::null(),
                            if right { 4 } else { 2 },
                            p,
                            if right { 1 } else { 0 },
                        ))?;
                    }
                    if v["action"] == "drag" {
                        let end = Point {
                            x: number(v, "to_x")? as f64,
                            y: number(v, "to_y")? as f64,
                        };
                        post(CGEventCreateMouseEvent(std::ptr::null(), 1, p, 0))?;
                        for i in 1..=12 {
                            let f = i as f64 / 12.0;
                            post(CGEventCreateMouseEvent(
                                std::ptr::null(),
                                6,
                                Point {
                                    x: p.x + (end.x - p.x) * f,
                                    y: p.y + (end.y - p.y) * f,
                                },
                                0,
                            ))?;
                            std::thread::sleep(std::time::Duration::from_millis(12));
                        }
                        post(CGEventCreateMouseEvent(std::ptr::null(), 2, end, 0))?;
                    }
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
                        CGEventSetFlags(e, flags);
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
