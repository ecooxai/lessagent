# Lessagent

A persistent local computer and coding agent written in Rust, with a browser UI, CLI, and MCP server. Codex authentication is the default provider. Light mode uses a pale top bar; Normal mode uses a dark top bar.

Read `Agents.md` before working on this checkout. Development and tests use `target/debug/lessagent` with an isolated `--port` and disposable `--data-dir`; release commands in Start are for production use.

## Start

Requires Rust 1.95 or newer, macOS or Linux, and Bash. Install the Codex CLI and sign in if using the default provider:

```sh
codex login
cargo build --release --locked
./target/release/lessagent
```

Running `lessagent` starts (or connects to) the persistent backend on default port **3210** and opens the native Lessagent service-monitor window as the main app window. `cargo run` uses this same default launch mode in development. The native window shows the browser UI URL at the top and provides an **Open in browser** button. Custom URLs added in the native monitor are persisted in `service-check.json` under the active data directory and loaded again whenever the window is reopened. `lessagent start` starts the backend without opening the native window; `lessagent tui` opens the terminal interface directly; `lessagent service-check --base-url URL` opens only the standalone native monitor. App HTTP and MCP HTTP endpoints are printed at startup; both share port 3210, with MCP at `/mcp`. Backend logs are appended to `server.log` in the data directory. Use `lessagent serve` to run the backend in the foreground.

Manage the backend and configure an optional browser password:

```sh
lessagent stop
lessagent restart
lessagent --passwd 'your-password'
lessagent restart --passwd 'new-password'
```

`stop` gracefully stops jobs and terminals and saves state; calling it again is safe. `restart` starts the backend again without opening an interface, or simply starts it when already stopped. It accepts the same `--passwd`, `--port`, and `--data-dir` options as start. Older running backends are upgraded automatically on launch; stopping an older version without a shutdown endpoint uses `lsof` to identify its listener and data-directory lock before sending SIGINT. `--passwd` sets or changes a persistent password, including when the backend is already running. Passwords are stored as salted Argon2 hashes. When a password is configured, the browser shows a sign-in form with setup instructions; a valid access token also permits entry.

In the TUI, use `/open /absolute/project/path` to open a workspace, Tab to switch workspaces, and Enter to submit a task. Page Up, Page Down, and the mouse wheel scroll output. `/stop` cancels the selected workspace’s latest running task. Escape, Ctrl-C, or `/quit` detaches the TUI while the backend continues running.

For the browser interface, open the URL printed at startup. Local browser access needs no token by default. Choose **Open folder**, enter an absolute project path, and enter a task. The browser assets are embedded in the binary; no Node installation or frontend build is needed to run it.

Keep the backend running. Closing or reloading the browser preserves running jobs and terminals. Stopping the backend stops its live processes; the next start restores workspaces, tabs, drafts, settings, job history, and the 30 newest terminal transcripts. Older terminal metadata and log files are deleted during startup. Interrupted jobs are marked interrupted and are not automatically resumed.

## Models and audio

In Settings, choose a provider and use **Discover models**:

| Provider | Authentication | Model |
| --- | --- | --- |
| Codex (default) | Existing `codex login` session | Discover models directly; blank selects the first available model |
| OpenAI | `OPENAI_API_KEY` or a saved key | Choose a tool-capable Responses API model |
| Claude | `ANTHROPIC_API_KEY` or a saved key | Choose a tool-capable Messages API model |
| Gemini | `GEMINI_API_KEY` or a saved key | Choose a model supporting function calls |

Keys can also be saved in Settings. Environment variables take precedence. Keys are stored locally with owner-only permissions and are never returned to the browser. Existing provider preferences are preserved across upgrades.

Codex requests go directly to the ChatGPT Codex Responses endpoint over HTTP using the access token and account ID from `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`). No Codex agent or app-server process is launched. Lessagent owns the model/tool loop and executes tool calls. File credentials are required (`cli_auth_credentials_store = "file"` in Codex configuration); keyring-only credentials are not supported. Expired credentials require running `codex login` again; Lessagent does not refresh or modify shared credentials. This is a local automation tool, not a security sandbox for untrusted tasks.

Transcription, text-to-speech, and live WebRTC voice use a separate **OpenAI API key**, even when the agent provider is Codex. Audio model IDs are configurable in Settings. Live voice requires microphone permission and an open browser tab. Live voice tools use the workspace selected when the call started.

## Workspaces and context

- **Normal:** sends only the active Agent tab's `agent/context/<session>/` contents and task instructions. The agent reads project files using tools and saves findings, replies, and test evidence into that directory. Every three task requests, the configurable compaction model (default `gpt-5.6-luna`, on the selected provider) summarizes the context. Original messages and evidence move to `agent/msgs/<job>/iteration-N/` after a valid summary is saved. Failed compaction retains the original context. Compaction API usage is included in task totals.
- **Light:** available only below **30k estimated project-file tokens** with a complete scan. Each request rebuilds the selected project files, respecting project `.gitignore` and configured exclusions, and includes every current file under `agent/output/`; `agent/output-history/` and `agent/continuity/` are ignored. The output tree is moved into history after each response before the next iteration directory is created. The previous **two AI replies** are sent separately with bounded text and action outcomes; older replies and the full chat transcript are not replayed. Supported images produced in one iteration are attached to the next request for review. The request viewer shows this window and its token estimate separately from files.
- Real Light deliverables live in `agent/output/<session>/<task>-<job>-<iteration>/`. That tree contains only files the task produced for review or use, such as compiled binaries, screenshots, and images. Actions, results, state, summaries, and command logs live in the internal `agent/continuity/<session>/` tree and are excluded from project context; older artifact snapshots move to `agent/output-history/`.
- Light mode periodically uses a tool-free progress checkpoint to summarize status, choose the next action, or finish verified work. Repeated unchanged terminal polls receive a stall signal. Verification runs only an AI-provided meaningful `<test>` command; there is no automatic placeholder test.
- Click **Project context** for all folders and files, with excluded files shown in gray and included entries sorted by token count. Text estimates use o200k_base. Image estimates scale to a maximum 2048px side and a 2500-patch budget using 32px patches at one token each. File totals exclude prompts, the two-reply window, file labels, tool definitions, and provider framing; request parts are estimated separately, and actual billing usage comes from provider responses.
- Files larger than 16 MiB and bundles larger than 64 MiB prevent Light mode. Unsupported binary files are not model context; audio and video are available as previews. Ensure sensitive files are ignored before sending project context to a provider.

The sidebar shows folder names; hover for the full path, or double-click a folder to choose Normal or Light mode, or Close to hide the workspace while preserving its history. The full workspace path appears above the tabs. Use **＋** beside the tabs to open a new Agent session, Files, Terminal, Git, History, Managed terminals, or Webview. Managed terminals are started by the agent and list running commands first, then idle sessions, with newer sessions first within each group. Tabs support an agent view, terminal grid, sandboxed webviews, settings, logs, text editing, and image/audio/video previews. Terminal cards have a 300px minimum width, split into additional sessions, resize vertically, and preserve their height. Sites that prohibit embedding can be opened in a separate browser tab.

All shell tools run in real PTYs maintained in backend memory. Output is also persisted, with a 256 KiB tail per terminal. Tool reads return the newest 24,000 bytes. There can be up to 32 live terminals. **Stop agent** cancels the model loop; use a terminal's **Stop** button to stop a command that is still running.

## Computer control

Enable mouse, keyboard, and screenshots in Settings.

- **macOS:** uses CoreGraphics and `screencapture`. Grant Accessibility and Screen Recording permission to the terminal/application hosting the backend.
- **Linux X11:** install `xdotool` and `scrot`, and run the backend in the desktop session.
- **Linux Wayland:** `grim` supports screenshots; mouse and keyboard injection currently require X11.

Supported actions include screenshot, move, click, drag, type, key, and scroll. HTTP/API and Agent-tab results return a screenshot's saved path and screen dimensions without embedding base64 image data; the MCP adapter exposes the same artifact as a dedicated base64 image content block for MCP clients. Commands execute with the backend user's operating-system permissions. File tools enforce workspace path boundaries; Bash and computer control intentionally have broader host access.

## CLI

Start the backend with `lessagent start`, then use:

```sh
lessagent open /absolute/project
lessagent run /absolute/project "Inspect this project and run its checks"
lessagent status
lessagent models codex
lessagent tool WORKSPACE_ID shell '{"command":"pwd","wait_ms":1000}'
```

Use `./target/release/lessagent` in place of `lessagent` unless the binary is on your PATH. `--port` and `--data-dir` work with every command; clients must match the server. Defaults are port `3210` and `$XDG_DATA_HOME/lessagent` or `~/.local/share/lessagent`. Environment alternatives are `LESSAGENT_PORT` and `LESSAGENT_DATA_DIR`.

State uses atomic file replacement, owner-only credential files, and a single-server lock per data directory. The HTTP server listens on all IPv4 interfaces (`0.0.0.0`), accepts requests using the server IP address or hostname, validates browser Origin, and rejects cross-site browser requests. Open `http://SERVER_IP:3210/` from another device; remote access requires a password configured with `lessagent --passwd PASSWORD`, or a valid token. Loopback browser API access needs no token unless a password is configured. Server administration requires the bearer token. The `/mcp` endpoint is public and requires neither a password nor a token, including remote connections. Optional URL fragment tokens are moved into browser session storage and removed from the address bar.

## MCP

### Server instructions, system information, and artifacts

Read `lessagent://server/instruction.md` first. The server advertises MCP resources and implements `resources/list`, `resources/read`, and an empty `resources/templates/list`. The resource contains live OS/CPU/GPU/RAM information, macOS screen resolutions in logical points and backing pixels, coding/computer-control instructions, and completion/output rules. It is available before opening a workspace and while computer control is disabled. Resource URIs are allowlisted; this endpoint never opens arbitrary files or fetches arbitrary URLs. Hardware fields that cannot be detected are explicitly unavailable. Resource subscriptions are not supported; re-read after a display/configuration change. Host information is visible to anyone who can reach the existing MCP endpoint; expose it only through trusted connections.

Tool-only clients can use the read-only `list_resources` and `read_resource` tools instead. Both require `summary`, as do all other MCP tools. Native protocol resource requests do not require a summary:

```json
{"jsonrpc":"2.0","id":1,"method":"resources/list"}
{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"lessagent://server/instruction.md"}}
```

Use project-root `output/` for important finished work: verified binaries, images, 3D files, renders, and other deliverables. Direct computer captures default to `output/computer/`; internal agent iteration artifacts keep their existing locations. Finish each task with a factual summary of changes, output paths, actual test results, and limitations.

Use the public `browser_open` tool for controlled Chrome and `app_open` for native applications. The restored managed-browser path reuses Lessagent-managed profile storage and never copies or modifies personal Chrome data. Reuse the returned exact `window_id` / `pid` with `virtual_pointer` and `virtual_keyboard`. See `docs/instruction.md`.

Image responses provide actual encoded `width`, `height`, `format`, and `image_metadata`, plus `screen_width`/`screen_height` for screenshots. The MCP image block carries `_meta["lessagent/image"]` and compatibility aliases for clients that forward those fields. Use those verified pixel dimensions for coordinates, not requested window sizes. Client-private inspector fields such as `fovea` are not fabricated; an adapter may still ignore extension metadata.

The focused macOS regression test verifies resource access, default and maximum-size windows, oversized-request rejection before launch, output paths, and image metadata through MCP and the browser API:

```sh
```


For a local MCP client that supports stdio, configure the absolute binary path:

```json
{
  "mcpServers": {
    "lessagent": {
      "command": "/absolute/path/to/lessagent",
      "args": ["mcp"]
    }
  }
}
```

The stdio process bridges to the running backend. Add matching `--port` and `--data-dir` arguments when using custom settings. Tools include workspace open/list, resource bridges, Bash/Python/PTYs, multi-file `get_files`/`send_files` transfer, image writing, and standalone GUI control. MCP intentionally does not expose `agent_run`, `agent_status`, `read_file`, `write_file`, the aggregate `computer` tool; the connected client orchestrates those operations with Bash/Python and the standalone primitives.

Clients supporting stateless MCP HTTP POST can use `http://127.0.0.1:3210/mcp` without authentication headers. MCP remains public even when a browser password is set. JSON-RPC notifications return HTTP 202. The implementation supports initialization, ping, tool discovery, and calls; it does not provide an SSE event stream. A cloud-hosted chat such as ChatGPT cannot reach your loopback address directly; connecting it requires a compatible trusted bridge outside this project's local server.

### Read-first guidance and per-call summaries

MCP initialization and tool descriptions tell clients to read the workspace-root `Agents.md` with `bash` or `python` before project work; try `AGENTS.md` if absent and read applicable nested guidance. `read_file` and `write_file` are not exposed over MCP.

Every MCP tool requires eight request-only observability fields: `summary`, `agent`, `model`, `main_task`, `current_task`, `progress`, `quality`, and `current_timestamp`. `summary` allows 1–500 Unicode characters so it can capture verified work completed so far, relevant mistakes/corrections when they occurred, and what the current tool call will do and why. Do not invent mistakes, claim unverified success, or repeat main task, current task, progress, or quality inside it. `progress` and `quality` are integer 0–100 values. Detailed guidance lives in `lessagent://server/instruction.md` instead of being repeated in every tool description. For example:

```json
{"workspace":"WORKSPACE_ID","summary":"Verified the target window and screenshot dimensions. Earlier I used stale coordinates, so I corrected them from the latest screenshot. This call scrolls the exact selected window to reveal the next section.","agent":"ChatGPT","model":"GPT-5.6 Sol","main_task":"Inspect background window","current_task":"Scroll to next section","progress":65,"quality":92,"current_timestamp":"2026-09-12T08:00:00-07:00","action":"scroll","window_id":123,"pid":456,"x":800,"y":500,"distance":10,"screen_width":1000,"screen_height":600}
```

Missing, malformed, whitespace-only, or over-500-character summaries are rejected before execution. Metadata is stripped before dispatch and recorded with sanitized arguments/output in Logs. Execution results remain under `structuredContent.result`, and `structuredContent.time_cost_ms` reports elapsed time.

MCP execution tools are `bash` (`command`) and `python` (`code`, Python 3), with `shell` retained as a Bash alias. Both start workspace PTYs and return `terminal_id`, `output`, `exited`, and `exit_code`. `wait_ms` waits up to 10 seconds; use `terminal_read`, `terminal_write`, and `terminal_stop` for ongoing commands. Each Python call starts a fresh interpreter; stdin stays available through `terminal_write`.

`virtual_pointer.distance` uses positive values for up and negative for down; legacy `delta` has the opposite sign. Input actions return a fresh automatic screenshot after the settling delay.

To send an image to the MCP server, call `write_image` with a standard image block nested in its JSON arguments:

```json
{"workspace":"WORKSPACE_ID","summary":"Image ready; save it","agent":"ChatGPT","model":"GPT-5.6 Sol","main_task":"Save supplied image","current_task":"Write PNG","progress":80,"quality":95,"current_timestamp":"2026-09-12T08:00:00-07:00","path":"images/input.png","image":{"type":"image","mimeType":"image/png","data":"BASE64_ENCODED_IMAGE"}}
```

The tool validates base64, image headers/dimensions, MIME type, extension, size (20 MiB maximum), and workspace path boundaries before writing. Supported formats are PNG, JPEG, GIF, and WebP. The structured result contains image metadata and a native MCP image block. Use Bash/Python for subsequent file reads. `get_screenshot`, `virtual_pointer`, and `virtual_keyboard` return PNG screenshot image blocks when they observe a GUI target; every successful virtual input includes a fresh automatic observation. Input images are file uploads; no model vision request is made by `write_image`.

The server negotiates MCP versions 2025-03-26, 2025-06-18, and 2025-11-25. The official Python SDK integration test covers HTTP and stdio:

```sh
uv run --with 'mcp>=1.20,<2' tests/mcp_client.py target/debug/lessagent
uv run --with 'mcp>=1.20,<2' --with pillow tests/native_app_virtual_tools.py target/debug/lessagent
```

The computer-control integration tests require macOS Accessibility and Screen Recording access. `browser_profile_virtual_tools.py` opens disposable managed Chrome profiles with browser_open and verifies reuse, persistence, and real virtual input. The local macOS `cargo test --locked` suite also runs `chrome_background_pointer_regression`, which Bash-launches a disposable Chrome profile and repeats the same trusted pointer sequence automatically. `native_app_virtual_tools.py` launches fresh Blender and Auri instances and verifies recipient-side state changes instead of treating event posting as acceptance. Neither test permits a global-input fallback.

## Development and validation

Use Cargo debug/dev builds for development and testing (`cargo build`, not `cargo build --release`; there is no standard `cargo debug` command). Start manual test servers with a different `--port` from 3210 and a disposable `--data-dir`; see `Agents.md` for complete commands. The integration harnesses choose their own isolated ports and data directories.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/smoke.py target/debug/lessagent
python3 tests/startup.py target/debug/lessagent
python3 tests/legacy_startup.py target/debug/lessagent
uv run --with 'mcp>=1.20,<2' tests/mcp_client.py target/debug/lessagent
node --check web/app.js
```

`python3 tests/legacy_startup.py target/debug/lessagent` verifies migration from an older backend without administration endpoints. `tests/workspace_picker.cjs` exercises the picker DOM with jsdom. See `Agents.md` for the isolated `jsdom@30.0.0` setup and commands for all `tests/*.cjs`; the thinking-menu test needs pointer-event handler support.

`python3 tests/startup.py` verifies automatic startup, app/MCP port logs, the startup menu, reconnection, TUI workspace opening and scrolling, detachment, stop/restart, token-free local access, password login and changes, password persistence, and public MCP access.

The smoke test starts an isolated backend and a deterministic Codex fixture. It checks authentication and origin boundaries, project context, model discovery, tool execution, cancellation, terminal survival without a browser, MCP over HTTP/stdio, and persistence after restart. It makes no paid API calls. Use the debug binary explicitly: `python3 tests/smoke.py target/debug/lessagent`.

Verified locally on macOS: Rust tests and lint, backend/CLI integration, actual Codex model discovery using the installed login, browser mode switching, split terminals, reload persistence, and permission-dependent background GUI control through managed Chrome, Blender, and Auri. Direct Codex generation, file editing, and the test-feedback loop were also exercised using the installed login. Audio calls have not been exercised. Linux checks are configured in CI; they have not been run on this macOS host.

The implementation uses Rust dependencies and plain HTML/CSS/JavaScript. `src/agent.rs` owns the agent loop, `provider.rs` provider translation, `context.rs` inventory/bundling, `terminal.rs` PTYs, `computer.rs` platform input, and `server.rs`/`mcp.rs` the interfaces. Release builds enable thin LTO and strip symbols. The macOS idle benchmark (`python3 tests/idle_performance.py target/release/lessagent DATA_DIR`) uses a disposable copy of saved data and checks CPU below 10% and physical memory footprint below 100 MB while polling state once per second.

Context ignore patterns are editable in Settings, with visible defaults and a reset button. Defaults exclude lockfiles, licenses, temporary files and common generated directories. These rules apply to context scans and agent bundles, while the Files browser still displays ignored files. Token and file-size displays use M units at 100,000 and above.

Files, Context and the workspace picker share a tree browser. The workspace picker starts with regular folders and provides a hidden-folder toggle. Its path input accepts absolute paths, `~/` paths, or paths relative to the current folder; typing offers fuzzy matches for folder names within the typed parent directory. Press Enter to navigate to a path, or choose a suggestion. Opened workspaces and the selected workspace are saved and restored across backend restarts. Expand folders with a click, enter with a double-click, and use the parent arrow to go up. Open folders refresh every 15 seconds while focused; newly discovered files appear first in blue for 30 seconds. The Files terminal button opens a folder terminal dock; navigating reuses an idle shell or creates a new one if the selected shell is busy.

The terminal uses a custom JavaScript screen renderer backed by the in-memory VT100 parser, without a frontend terminal dependency. It supports ANSI colors, direct keyboard input, paste, IME input, resize, scrollback and mobile key controls. Selections longer than four characters are copied after three seconds when browser clipboard permissions allow; an explicit Copy button is available as well.


Each completed task displays uncached input tokens, output tokens, cached input tokens, and elapsed wall time. Totals include every successful model step. Missing usage is marked unavailable or partial; older saved tasks retain unavailable metrics.

Opening a workspace creates Terminal, History, and Git tabs. History lists saved tasks with their prompt, reply, events, and metrics; clicking a task opens its own read-only session tab. Titles longer than 25 characters display their first 15 and last 10 characters, separated by an ellipsis; hovering shows the full title.

The Git tab has Changes and Branches subtabs. Changes lists files with added/deleted line counts; selecting a file displays its staged and unstaged diff. Branches lists local and remote branches from the last fetch; selecting a branch shows commit information, ahead/behind counts, and its committed diff against the current branch. Switch to the selected branch or create and switch to a new branch. Discard all changes requires confirmation and resets tracked files and removes untracked files; ignored files remain. Git management requires opening the repository root and is blocked while a Lessagent task is running. Terminal commands can still modify the repository independently.

`LESSAGENT_CODEX_BASE_URL` overrides the Codex HTTP endpoint for trusted gateways or local test fixtures. It receives your Codex authorization token; only configure an endpoint you trust.

Thinking effort is configurable in Settings and beside Send. Press, slide to an effort, and release to send immediately. A plain click opens the menu for three seconds before sending with the last effort; Escape cancels. The footer shows effort, latest quality score, and Codex five-hour/weekly remaining limits when available. Unsupported model/effort combinations return a clear error.

The agent requests a 0–10 self-assessed completion score and a Bash test command after each action iteration. Tests run with a 120-second timeout; their text and result metadata live in `agent/continuity/<session>/`, while image evidence and other real deliverables stay under `agent/output/<session>/<task-name>-<job-id>-<iteration>/`. The next request receives test results and bounded artifact contents. Completion requires a passing test and a score at or above the Settings threshold (default 9), with no pending tool calls. The model can explicitly stop using `<done><done>`. Cancellation and the configured maximum step count also bound execution. A model score is self-assessment, not an independent quality guarantee.

Agent messages offer an expandable “What was sent to AI” view with task/system/iteration prompts, model settings, and context file names (file contents stay hidden). Agent tabs keep separate messages, drafts, and context folders; one task runs per workspace at a time. Light desktop actions use `<computer>` JSON tags; take a screenshot first and pass its returned pixel dimensions with pointer coordinates. On macOS, Lessagent converts those screenshot pixels to Quartz logical points for Retina displays. Screenshots are saved as real artifacts under the current `agent/output` iteration and are attached to the next request. Model discovery runs at backend startup and when an Agent tab opens. Codex discovery uses the installed CLI version (only `codex --version`, never the CLI agent); `LESSAGENT_CODEX_CLIENT_VERSION` can override it.

Large job events (over 16 KiB) are archived under `events/` in the data directory. State and UI polls contain small references; the History/Agent view loads original event details when **Load details** is clicked. Existing events migrate automatically on startup. Archived terminal screens are reconstructed only for a view request and released afterward; live PTYs continue maintaining their virtual screen while their tabs are inactive. The web terminal waits for output notifications, sends input directly to the PTY, and repaints changed rows only while focused. Background project scans do not run for terminal tabs.

### Standalone GUI tools

`get_screenshot` captures a read-only target image and metadata without an action argument. `virtual_pointer` handles move/click/drag/scroll; `virtual_keyboard` handles `type`+`text` or `key`+`key`. Every successful virtual input returns a fresh automatic screenshot image. `list_windows` lists existing exact targets. Use `app_open` for native apps and `browser_open` for Chrome, then reuse the returned exact target IDs. Create a fresh profile or additional instance only when explicitly requested. Each tool retains normal workspace/summary and permission requirements. The aggregate `computer` MCP tool has been removed; internal Light-mode compatibility still uses a private aggregate executor and is not part of MCP discovery.

For ordinary native-app work, use `app_open` with `app` (installed name, bundle ID or absolute `.app` path) and `new_instance:false`. This reuses one unambiguous window or launches normally if the app is not running; ambiguity returns an error directing the client to `list_windows`. The restored launcher's `new_instance:true` default requests another instance, not a fresh profile. The normal app profile/session is preserved, and the tool returns exact IDs and an automatic screenshot. The same required metadata, including a 1–500-character `summary`, applies.

Native window coordinates come from exact-ID Accessibility geometry, not Stage Manager's distorted thumbnail bounds. Ordinary window capture uses ScreenCaptureKit; shelved windows can use a perspective-corrected **low-resolution thumbnail**, explicitly marked by `capture_quality`, `perspective_corrected` and `capture_backend` in both result and MCP image metadata. No activation or global input fallback is used to improve capture quality. See [background-control.md](docs/background-control.md) for scope, examples, and limitations.

### macOS background GUI control

The standalone MCP GUI tools share one native control backend with the CLI and agent runtimes; Light mode keeps a private aggregate executor for its internal `<computer>` protocol. Enable computer control in Settings and grant macOS Accessibility and Screen Recording permissions to the process running Lessagent.

Use the public `browser_open` tool for controlled Chrome and `app_open` for native applications. The restored managed-browser path reuses Lessagent-managed profile storage and never copies or modifies personal Chrome data. Reuse the returned exact `window_id` / `pid` with `virtual_pointer` and `virtual_keyboard`. See `docs/instruction.md`.

Chrome page input uses the registered managed DevTools channel created by `browser_open`. Unmanaged Chrome is screenshot-only; no experimental native Chrome or shared/global pointer fallback is used. Other applications retain the exact-process/window native input path.

Adoption records live in the backend data directory and can be reused while the exact browser target remains valid. They refer to the profile Chrome was actually launched with; Lessagent does not create a second persistent copy of the user's original profile.

Only page content is controlled through this channel. Browser title bars,
toolbar menus, docked developer tools/side panels, and multiple tabs in the same
managed window are rejected rather than risking the wrong target. Use browser_open for another controlled window to navigate independently. Other native applications continue
to use process-directed input and require application-specific verification.

For an existing native application, call `list_windows` first, select a window, and send both its `window_id` and `pid` on every targeted `get_screenshot`, `virtual_pointer`, and `virtual_keyboard` call. Input without a target
is **rejected on macOS**, and explicit `mode:"desktop"` is rejected too. A typo,
missing owner, stale window, or incomplete drag must never move the shared
pointer. There is no macOS global input backend. An initial untargeted
screenshot remains available for read-only desktop discovery.

Use the first object as `get_screenshot` arguments, pointer-action objects as `virtual_pointer` arguments, and type/key objects as `virtual_keyboard` arguments:

```json
{"mode":"background","window_id":123,"pid":456}
{"action":"click","window_id":123,"pid":456,"x":440,"y":194,"screen_width":2000,"screen_height":1500}
{"action":"type","window_id":123,"pid":456,"text":"Hello from the background"}
{"action":"key","window_id":123,"pid":456,"key":"cmd+a"}
{"action":"drag","window_id":123,"pid":456,"path":[[500,220],[630,518],[291,372],[709,372],[370,518],[500,220]],"duration":3}
{"action":"scroll","window_id":123,"pid":456,"x":800,"y":500,"distance":-10}
```

Replace example IDs with discovery results. Coordinates start at the **whole
window's top-left**, including its title bar. Without screenshot dimensions they
are logical macOS points. With both dimensions they are pixels of that window's
screenshot, not a desktop screenshot. Negative/out-of-window points, malformed
paths, invalid dimensions, buttons, modifiers, and durations fail before input.
A drag may use `x,y,to_x,to_y`, endpoint aliases, or a 2–512-point `path`. A missing
destination axis keeps the starting coordinate. `duration` is 0.05–5 seconds;
`duration_ms` is 50–5000 milliseconds; do not supply both. Right-button clicks
and drags preserve right-button identity. Every stroke has one content-facing
down/up pair; release is protected by cleanup and occurs after its last move.

For non-browser native applications, canvas/viewport pointer actions and keyboard events use an isolated recipient-local process/window path. Ordinary left-clicks on actionable Accessibility controls can instead use exact-window `AXPress`, which lets WebKit/Catalyst-style apps accept semantic background clicks without activation. One requested click is never sent through both routes; right-clicks, drags, moves, and scrolls remain process-window events. Neither path warps the system cursor, injects Command into ordinary clicks, activates the target through LaunchServices, or falls back to global HID input. Click a text field before typing. Protected surfaces and application-specific input handling still require recipient-side verification; posting an event is not acceptance proof.

A light blue virtual pointer (`#7DD4FF`) marks targeted input. It turns dark blue
(`#2563EB`) during a left press/drag and returns to light blue after release.
The nonactivating overlay ignores mouse events and cannot take keyboard focus.
`show_pointer:false` hides it. After **10 seconds of inactivity**, the colored
fill becomes transparent while a faint outline stays visible. After **30 seconds**,
the pointer disappears. New mouse or keyboard input restores it and restarts
the timer; screenshots and window discovery do not count as activity. The live
overlay and screenshots use the same position and idle state. Metadata includes
`pointer.phase`, `fill_alpha`, `idle_seconds`, and `panel_visible`.

Take one explicit screenshot for the initial view. After each successful move,
click, drag, type, key, or scroll, Lessagent waits two seconds and automatically
captures the same target. Agent/Light captures go into the current `agent/output`
iteration; direct calls use `output/computer`. MCP returns a native PNG
image content block alongside text metadata. HTTP and normal-agent results keep
binary image data out of JSON text. Input and observation are serialized across
concurrent tool calls. If capture fails after input, `screenshot_error` preserves
that distinction: recapture rather than blindly repeating the input.

Results identify the transport: `delivery:"chrome-devtools"` for managed Chrome with `native_input_events_posted:0`, `delivery:"process-window"` for native recipient-local events, or `delivery:"accessibility-window"` for a semantic AXPress click. They include `input_events_posted`, `responder_lease`,
`before` and `after` desktop state, and the automatic screenshot's `desktop` state. This exposes both immediate and delayed foreground changes.
Human mouse movement can change sampled cursor coordinates; the controller does
not restore/warp over the user's own movement. Native input requires a valid exact Accessibility window that is not minimized.
Stage Manager shelf visibility is not a substitute for native window identity.
The result also records target window presentation before/after input; compare
it as well as the foreground PID when validating background behavior.

Native routing uses runtime-resolved AppKit/Quartz window-event functions.
Unavailable support returns an error instead of silently using desktop input.
Swift is required only at build time; the compiled helper is embedded in release
builds. One serialized helper owns the nonactivating virtual pointer across actions.
Its two one-shot idle timers do not poll focus. It exits when the backend closes
the pipe; a helper failure never replays an action or uses a global input API.

macOS app launches default to background through `open -g`; Blender additionally uses `--no-window-focus`. Preserve the user's existing Chrome profile and follow `docs/instruction.md` for the exact new-window launch recipe. `mode:"background"` names the isolated transport and can also control an already-foreground target.

Native Chrome clicks/keys use recipient-local focus notifications without restoring global focus. Native Chrome drag is rejected before input because shared pressed-button state cannot be changed safely by this transport; an already-approved DevTools target supports complete independent gestures. `list_windows` reports these action capabilities. Native supplementary Unicode and background Command shortcuts also require approved DevTools; unsupported requests fail before input rather than silently dropping text or commands. Successful posting is not proof of application acceptance; the MCP matrix in `tests/macos_app_input.py` checks actual browser events and Blender scene state.

**macOS is background-only.** All mouse, keyboard, drag, and scroll actions
require `window_id` and `pid`; even an explicit `mode:"desktop"` is rejected.
The macOS global-input implementation has been removed, not merely bypassed.
Read-only untargeted screenshots remain available for discovery. Linux X11
desktop input is unchanged.

Run the background-control regression with the official MCP SDK:

```sh
cargo build --locked
```

The test starts disposable Chrome and Lessagent instances. It exercises real
buttons, Unicode/emoji input, text selection and shortcuts, a range slider,
left/right clicks and drags, horizontal/vertical/curved/fast/repeated strokes,
actual scrolling and direction, invalid requests, and screenshot delivery.
Both MCP transports and normal HTTP call the same engine. It validates trusted
events, button masks, exact endpoints and path geometry, no duplicate click,
no stuck button, and the target staying out of the OS foreground through the
observation delay. Failure is saved as failure, not hidden by a foreground waiver.
Evidence is written under `output/background-control`, including `report.json`
and the real window screenshots. The fixture's JavaScript only observes trusted input and implements the controls
and drawing. Managed browser input uses Chrome's `Input` protocol; no DOM event
handlers or canvas drawing commands are injected. Viewport geometry is read with
a fixed, read-only expression. `LESSAGENT_CONTROL_REPORT_DIR` selects a separate
evidence directory for concurrent test runs. `LESSAGENT_NATIVE_PROBE=1` preserves
the diagnostic reproduction of unsupported native-only Chrome input, not a
promise that native-only Chrome passes these independence checks.

### Interactive pointer accuracy lab

```sh
python3 tests/control_lab.py --port 4174
```

Open `http://127.0.0.1:4174/` in a separate background Chrome window. The lab
shows actual event coordinates, expected targets, measured error, trusted-event
status, a drag surface, a slider, keyboard input, and two independent scroll
regions. `/events` returns the measurements as JSON. `/plan` accepts expected
client-coordinate guides only; it never synthesizes DOM or native input.

The native regression also checks a nine-position grid at logical, Retina, and
resized-screenshot scales, cross-track drag error, scroll target isolation, and
the live overlay metadata at 9, 10.4, 29, and 30.4 seconds of inactivity. When
macOS permits `/usr/sbin/screencapture -l` to capture the nonactivating pointer
panel, the test also verifies its actual pixels; if that OS utility rejects the
panel, phase/fill/visibility metadata plus target screenshot overlays remain the
required lifecycle oracle. Results are saved in `output/background-control/report.json`.

Managed Chrome geometry is obtained from the verified browser window, not from
Quartz presentation bounds during Mission Control or Space animations. Returned
PNG dimensions always describe the actual image, including scaled captures;
logical dimensions continue to describe the full window. The isolated Chrome
channel can address its window on another Space when native capture remains
available. Native PID input still requires an onscreen target. No channel falls
back to global input.

Chrome page input uses the registered managed DevTools channel created by `browser_open`. Unmanaged Chrome is screenshot-only; no experimental native Chrome or shared/global pointer fallback is used. Other applications retain the exact-process/window native input path.

Controlled Chrome observations capture the actual page surface, not macOS's
Stage Manager/Mission Control thumbnail. The image preserves window-relative
coordinates by reserving the real browser-toolbar inset as a labeled neutral
band. That band is explicitly **not a captured browser toolbar** and is not
interactive. Metadata identifies `capture_backend: "browser-surface-window-frame"`
and `browser_chrome_captured: false`. This avoids skewed or tiny desktop
representations being misinterpreted as a full-resolution control image. Native
non-Chrome captures use bounded read-only retries; input is never replayed.
