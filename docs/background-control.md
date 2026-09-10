# Background GUI tools

Use `list_windows` to select an existing window, `get_screenshot` to observe it, `virtual_pointer` for pointer actions, and `virtual_keyboard` for text/shortcuts. The aggregate `computer` MCP tool has been removed; these standalone tools are the public GUI surface. All MCP tool calls require a workspace and a nonblank `summary`; computer control must be enabled. macOS native input requires Accessibility and capture requires Screen Recording.

## Tool boundaries

| Tool | Arguments beyond workspace/summary |
| --- | --- |
| `get_screenshot` | Optional window_id/pid, capture_path, show_pointer. No action or input fields. Omitting the target gives a read-only desktop overview. |
| `virtual_pointer` | action: move/click/drag/scroll; exact window_id/pid on macOS; relevant coordinates, button, duration/path or scroll distance. |
| `virtual_keyboard` | action:type with text, or action:key with key; exact window_id/pid on macOS. Never both text and key. |
| `list_windows` | None. Lists existing windows without activation. |
| `app_open` | app: installed name, bundle ID or .app path; optional new_instance and capture_path. |
| `browser_open` | HTTP(S) URL; optional logical-point width/height and capture_path. |

Both schema validation and the shared executor enforce standalone action boundaries. A screenshot call cannot be turned into input by adding an action. Input automatically returns a fresh screenshot. A screenshot failure after input is reported separately; observe again rather than replaying the input.

`get_screenshot` returns the same screenshot artifact fields used by automatic observations: actual pixel dimensions, pointer overlay, capture_path behavior, image metadata, and a native MCP image block. Every successful `virtual_pointer` and `virtual_keyboard` action also returns a fresh automatic image block. Use **the returned image width/height** as `screen_width`/`screen_height` with pointer pixel coordinates. These are not logical window sizes or display dimensions.

## Why Chrome is different

`browser_open` reuses one persistent Lessagent-managed Chrome profile and its loopback DevTools process when available, creating a separate controlled window for each call. Cookies, localStorage, and other profile-backed state persist across calls. Input is dispatched into the selected page with independent virtual pointer/button state; page capture reads the browser surface. This is not JavaScript `dispatchEvent`, global mouse injection, or attachment to the user's regular browser. Existing managed windows can be reused by ID. The normal unmanaged Chrome profile remains read-only; it is never restarted or debug-enabled.

Native apps do not share Chrome's DevTools protocol. Canvas/viewport input and native keyboard input use exact-process, exact-window macOS events. Native keyboard events carry the Cocoa window number, which event-driven applications such as Blender use to associate input with their window. Some WebKit/Catalyst-style apps ignore process-addressed background clicks on semantic controls; for an ordinary left-click Lessagent first resolves the exact target AX window and uses `AXPress` only when the clicked element is an actionable Accessibility control. If there is no eligible semantic control, the click follows the existing process-window event path. Right-clicks, drags, moves, and scrolls always use the process path. A single requested pointer action is never sent through both routes. No synthetic application-activation or make-key lease is used.

## Existing and new native windows

`app_open` requests a non-activating LaunchServices launch. Blender additionally receives its own `--no-window-focus` option and a display-fitted window geometry. `new_instance:false` reuses exactly one unambiguous existing titled window without reopening it; ambiguous multi-window applications require selection through `list_windows`. Arbitrary shell commands and user-supplied launch arguments are not accepted. Chrome must use `browser_open`.

An application may ignore nonactivation on startup. Inspect `focus_changed`; the controller does not hide such a change by activating the previous app or restarting the target. Input itself never uses an activation fallback. Minimized native input targets are rejected. Exact window identity and Accessibility geometry, not the visibility of the Stage Manager shelf thumbnail, determine the native target.

## Stage Manager capture and coordinates

Quartz can report the bounds of a perspective thumbnail instead of the actual native window. Lessagent matches Accessibility windows to the exact WindowServer ID and uses the true position and size for input. It retains presentation geometry separately for the virtual pointer overlay and diagnostics.

For ordinary windows, ScreenCaptureKit captures the native window without a desktop composite. Some shelved windows expose only a perspective thumbnail. Lessagent fits the thumbnail's alpha-mask edges, rectifies its perspective, and preserves the actual native aspect ratio. Ambiguous/transitional edges fail rather than producing uncertain click coordinates. This is a **low-resolution observation**, not a reconstructed high-resolution image.

Inspect `capture_quality:"thumbnail"`, `perspective_corrected:true`, and `capture_backend:"native-window-rectified-thumbnail"`. These also appear in `image_metadata` and the MCP image block's `_meta["lessagent/image"]`. A small image may be inadequate for reading fine text. Capture never moves, expands, or activates a window to improve resolution.

## Validation and limits

Check foreground PID **and window presentation**, not just process focus. Native results include target_window_before/target_window_after, and screenshots include window_presentation. Window expansion in Stage Manager can occur independently of the foreground PID. Human input can also change sampled state; never warp the physical pointer or restore focus over the user's actions.

An acknowledged process event means it was posted, not that every application accepted it; semantic `AXPress` success likewise needs application-state validation when correctness matters. Menus, custom responder chains, cursor-locking applications, secure/protected surfaces, other spaces and application-specific native input need their own validation. Chrome's full independent page channel is not a universal protocol for every native application. Unsupported behavior must not trigger global input, activation, or an automatic destructive retry.

Development and validation use debug builds only, with a separate free `--port` and disposable `--data-dir`. Do not restart the backend serving the current MCP connection. Source changes and tests do not deploy the running production backend.

## Verified native compatibility

The debug integration `tests/native_app_virtual_tools.py` exercises the public standalone MCP surface against fresh Blender and Auri instances. Blender is validated through visible background-click change plus a keyboard-driven 3D-viewport sidebar toggle, avoiding dependence on object selection state. Auri is validated with a read-only Accessibility oracle: System and Terminal tab selection, command-field focus, and the typed marker must all change while a different application remains foreground. Every virtual pointer/keyboard call must include a fresh PNG image block.
