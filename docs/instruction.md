# Lessagent MCP instructions

## Start here

Read this server resource first through MCP `resources/read` at `lessagent://server/instruction.md`; tool-only clients may use `list_resources` and `read_resource`. It contains the current host CPU, GPU, RAM, display geometry, tool rules, and project workflow. Re-read it and refresh `tools/list` after display or server changes.

Open the requested project with `workspace_open` or recover it with `workspace_list`. Before project work, use `bash or python` to read the complete workspace-root `Agents.md`; try `AGENTS.md` if absent, report when neither exists, and read applicable nested guidance before editing a subdirectory.

## MCP call metadata

Every MCP tool call requires `summary`, `agent`, `model`, `main_task`, `current_task`, `progress`, `quality`, and `current_timestamp`. `summary` is a 1–500 Unicode-character observability paragraph. Use it to say what has been verified or completed so far, mention any relevant mistake, failed assumption, or correction from earlier work, then use a blank line (`\n\n`) before stating what this specific tool call will do and why. Do not invent mistakes, do not claim the current call succeeded before seeing its result, and do not repeat `main_task`, `current_task`, `progress`, or `quality` because those have standalone fields. `progress` and `quality` are integers from 0–100. Use an ISO 8601 timestamp when possible. Metadata is stripped before execution and must not contain secrets.

## Coding and shell workflow

Inspect the working tree first and preserve unrelated changes. Use `bash`, `shell`, or `python` for source inspection and text editing; MCP intentionally does not expose generic `read_file` or `write_file`. Commands execute on the host with the user's permissions, so bound large reads and scope searches to the project.

Use debug builds and tests for development: `cargo build --locked`, `cargo test --locked`, and `target/debug/lessagent`. Do not use release builds unless explicitly requested. Start test services on a different `--port` with a disposable `--data-dir`; never stop, replace, or reuse the backend/data directory serving the current MCP connection.

Commands can return `terminal_id`. Use `terminal_read` for bounded follow-up, `terminal_write` for intentional input, and `terminal_stop` only for processes started for the task. `wait_n` is the simple MCP pause tool.

## File transfer

Use `get_files` and `send_files` when files themselves must cross the MCP boundary. They support batches of 1–64 files and a 24 MiB raw-data batch limit.

`get_files` accepts workspace-relative `paths`. PNG/JPEG/GIF/WebP files are returned as native MCP image blocks, audio files as native MCP audio blocks, and video, PDF, ZIP, Blender/3D, and other files as embedded resource blobs. Structured output also reports each path, MIME type, byte size, and media kind.

`send_files` accepts `files`, each containing a workspace-relative `path`, base64 `data`, and optional `mimeType`. Lessagent decodes and path-checks the complete batch before writes begin, rejects duplicate/escaping paths, then writes with atomic file replacement. Use it for images, audio, video, documents, archives, 3D files, executables, and arbitrary binary data. `write_image` remains a single-image convenience tool; prefer `send_files` for mixed or multi-file transfer.

Use Bash/Python rather than file-transfer tools for ordinary source-code reading and editing when raw file transport is not needed.

## Browser control

Use the `browser_open` tool to create a controlled background Chrome window. Do not replace this workflow with a raw shell launch or CLI bridge: the tool creates and registers the exact managed page used by `virtual_pointer` and `virtual_keyboard`.

Pass an HTTP(S) `url`, with optional `width` / `height` (default **1000 by 600 logical points**, fitted to the display) and `capture_path`. Include a short purpose query such as `?purpose=<modelname>+<purpose-for-this-window>` or `&purpose=...` for a URL that already has a query.

By default, each new window reuses the existing persistent **Lessagent-managed Chrome profile** and its running Chrome process when available. This restores the pre-removal managed-browser workflow. The managed profile is separate from the user's normal Chrome profile: it is not a copy of that profile and must not be described as preserving its logged-in accounts. Existing managed cookies and storage are retained. `new_profile:true` explicitly creates a separate blank managed profile. Never copy, reset, trim, merge, delete, or change Sync settings for personal Chrome data.

`browser_open` returns `window_id`, `pid`, `control_available`, profile-reuse information, and an automatic screenshot. Use those exact IDs and the image's actual `screen_width` / `screen_height` with the virtual input tools. The browser-local DevTools channel maintains independent button state for moves, clicks, continuous drags, scrolling, text and shortcuts. It never moves the shared physical pointer or activates the app as a fallback.

Unmanaged Chrome windows are screenshot-only: open a registered window with `browser_open` rather than retrying native input or silently enabling debugging on a personal profile. Only page content is supported in managed windows; the neutral toolbar band in screenshots is explicitly uncaptured and must not be clicked. Use one tab per managed window. After `screenshot_error`, take a read-only screenshot instead of replaying input.

## Opening a new window

For a new controllable Chrome window, call `browser_open`; its normal mode reuses the persistent managed profile. Use `new_profile:true` only for an explicitly requested fresh profile. For an existing registered window, select its exact `window_id` / `pid` with `list_windows` and take `get_screenshot` before input.

For native apps, use the restored `app_open` tool with `app` set to the installed application name, bundle ID or absolute `.app` path. The restored launcher defaults to a new instance; set `new_instance:false` to reuse one unambiguous existing window. Prefer reuse for ordinary work. It never restarts an existing app. When multiple windows exist, select the intended one with `list_windows` instead of guessing.

## Native application control

`app_open` requests a non-activating LaunchServices launch and returns exact IDs plus an automatic screenshot. For ordinary native-app work, pass `new_instance:false`: it reuses one unambiguous window, or launches normally if the app is not running. Both modes use the app's normal/default profile or session; a new instance is not a new profile. Blender additionally receives `--no-window-focus` and display-fitted geometry. Chrome must use `browser_open`, not `app_open`. Other apps may ignore nonactivation: inspect `focus_changed` and report that rather than hiding it by restoring foreground focus.

`virtual_pointer` and `virtual_keyboard` retain the pre-removal exact-process/exact-window native route for applications such as Blender. Exact-ID Accessibility geometry maps screenshot pixels to logical coordinates. There is no synthetic activation/responder lease or global-pointer fallback. Native windows must accept background input; verify the actual application result, not merely an event-post count. Minimized, ambiguous or stale targets must fail without input.

## Blender modeling context

Before keyboard input, use `virtual_pointer` move/click inside the intended editor (3D viewport, Console, Outliner, or text field). Lessagent keeps a separate virtual position for each native window and owner. Blender can refresh its internal cursor after a modal transform; keyboard calls restore this virtual position with one process-local move before sending keys. This does not click, activate Blender, or move the physical pointer. After a window resize or helper restart, select the editor again; missing/stale context fails before typing rather than guessing.

Use `button:"middle"` on a drag to orbit and `modifiers:["shift"]` with a middle drag to pan. Pointer modifiers accept `shift`, `ctrl`, `alt`, `cmd`; they are independent of physically held keys. Keyboard names include `numpad0` through `numpad9`, `numpaddecimal` (frame selected), `minus`, `period`, and the existing modifier chords. Numeric modal values can be entered with `type`, followed by `key:"enter"`. Verify object/mesh state rather than interpreting `ok:true` as application acceptance. Stage Manager thumbnails remain explicitly low-resolution; their actual pixel dimensions must accompany pointer input.

## Window input and screenshots

Use `get_screenshot` for the initial view or recovery from `screenshot_error`. `virtual_pointer` supports move, click, drag, and scroll. `virtual_keyboard` supports either `type` with `text` or `key` with a key chord. Successful virtual input waits three seconds after dispatch, then returns a fresh automatic screenshot; inspect it before the next action. The native top-right keyboard notice shows DOWN, UP and CLICK for a completed key tap, never takes focus, and hides five seconds after the last event. Text entry displays only its character count to avoid exposing sensitive text. Screenshots and window listing do not restart its timer.

Use the actual returned image pixel width/height for coordinates. Stage Manager may yield an explicitly marked perspective-corrected thumbnail; honor `capture_quality`, `capture_backend`, and `perspective_corrected` instead of assuming full-resolution capture. Scroll uses `distance` with positive values up and negative values down.

## Verify an input session

`mode:"background"` selects exact-window input, not a requirement that the recipient currently be behind another app. Use the same exact `window_id` / `pid` when the target is already foreground. Never switch to desktop/global input to make a foreground target work.

Use the screenshot's actual pixel dimensions in `screen_width` / `screen_height`; coordinates include the window frame and title bar. Do not mix Retina backing pixels with logical points or reuse a different window's screenshot dimensions. Refresh the target after a window closes, is replaced, or changes size. Minimized or stale targets must be rejected rather than guessed.

After each input, inspect both the fresh screenshot and the intended app result: text changed, a sidebar toggled, a shape was drawn, or an object moved. `ok:true` and `input_events_posted` only confirm dispatch, not application acceptance. Check foreground PID, `focused_window_id`, front-window identity, target presentation, and physical cursor independently; a Stage Manager window can expand without changing foreground PID. Never replay an input just because the automatic screenshot failed: request `get_screenshot` instead.

For server development, `cargo test --locked` runs the local macOS managed-Chrome pointer regression automatically. It opens a disposable test browser through the production launcher, repeats fixed gestures, and checks recipient events. Full MCP regressions are `tests/computer_background.py`, `tests/browser_profile_virtual_tools.py`, and `tests/native_app_virtual_tools.py`, always using the debug binary and separate backend ports/data directories.

## Deliverables and completion

Save important finished outputs under the active project directory's `output/`. Keep source edits in their normal locations and Cargo intermediates in `target/`. Put validation evidence in a named subfolder rather than overwriting unrelated output.

After finishing, provide a factual final summary covering what changed, where outputs are, which checks actually ran and their results, and any remaining limitations. Source changes do not deploy or restart the currently running backend.

## Service monitor alerts

Active failures appear first with red backgrounds and white text. Pause/Resume is immediately left of Remove for custom checks. Pausing stops scheduling and stale results must not re-alert. While the monitor is backgrounded, only the small top-right alert is shown; it disappears when current unpaused checks recover or are paused/removed. Clicking that alert opens the full monitor.

## Verify the deployed backend

`lessagent build-info` prints the identity compiled into that binary. `/health` reports the running backend’s `pid` and `build` identity; native GUI results include `backend_build`, including the embedded native-helper identity. Compare these values after an explicitly authorized restart instead of assuming a successful build updated an already-running process. `source_id` is a reproducible change fingerprint, not a security hash.
