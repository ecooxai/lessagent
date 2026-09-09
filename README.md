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

Running `lessagent` starts a persistent backend on default port **3210**, then prompts you to open the interactive terminal interface, open the browser UI, or leave the backend running without opening either. If the backend is already running, it connects to it. `lessagent start` starts the backend without a menu; `lessagent tui` opens the terminal interface directly. App HTTP and MCP HTTP endpoints are printed at startup; both share port 3210, with MCP at `/mcp`. Backend logs are appended to `server.log` in the data directory. Noninteractive launches start the backend and print connection details without entering the TUI. Use `lessagent serve` to run the backend in the foreground.

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

`browser_open` now defaults to **1000 × 600 logical points**. Both it and `computer` action `browser_open` advertise maxima based on the current primary display's logical resolution, and revalidate before launching Chrome. Windows are fitted and centered within its visible work area (excluding menu bar/Dock); `browser_size` reports requested/actual/maximum sizes. These are window dimensions, not Retina screenshot pixels. Refresh `tools/list` after changing resolution.

Image responses provide actual encoded `width`, `height`, `format`, and `image_metadata`, plus `screen_width`/`screen_height` for screenshots. The MCP image block carries `_meta["lessagent/image"]` and compatibility aliases for clients that forward those fields. Use those verified pixel dimensions for coordinates, not requested window sizes. Client-private inspector fields such as `fovea` are not fabricated; an adapter may still ignore extension metadata.

The focused macOS regression test verifies resource access, default and maximum-size windows, oversized-request rejection before launch, output paths, and image metadata through MCP and the browser API:

```sh
uv run --with 'mcp>=1.20,<2' tests/browser_geometry.py target/debug/lessagent
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

The stdio process bridges to the running backend. Add matching `--port` and `--data-dir` arguments when using custom settings. Tools include workspace open/list, agent run/status, file operations, PTYs, and computer control.

Clients supporting stateless MCP HTTP POST can use `http://127.0.0.1:3210/mcp` without authentication headers. MCP remains public even when a browser password is set. JSON-RPC notifications return HTTP 202. The implementation supports initialization, ping, tool discovery, and calls; it does not provide an SSE event stream. A cloud-hosted chat such as ChatGPT cannot reach your loopback address directly; connecting it requires a compatible trusted bridge outside this project's local server.

### Read-first guidance and per-call summaries

MCP initialization and every tool description tell clients to read the workspace-root `Agents.md` with `read_file` before project inspection, commands, edits, or delegation. Try `AGENTS.md` when the first spelling is absent, and report when neither file exists. Read the whole file using `has_more` / `next_offset` and check applicable nested guidance before working in a subdirectory. Workspace open/list results repeat this guidance. This is a client instruction, not a server-enforced acknowledgement gate.

**Every MCP tool now requires `summary`**, including workspace open/list and agent run/status. Supply a concise, nonblank paragraph of at most 1000 Unicode characters explaining what this particular call will do and why, without secrets or unverified success claims. For example, after opening a workspace:

```json
{"workspace":"WORKSPACE_ID","path":"Agents.md","summary":"Read the project instructions before inspecting code or running tests."}
```

Missing, non-string, whitespace-only, and overlong summaries are rejected before execution with `isError: true`. A valid summary is returned as a labeled client-intent text paragraph and `structuredContent.summary`, including when execution fails. Execution results stay under `structuredContent.result`; image content blocks remain separate. The adapter removes summary metadata before dispatch, so it is not executed as code or added to a delegated prompt. Direct CLI/HTTP and internal agent tool contracts are unchanged. Reconnect clients or refresh `tools/list` after deploying the updated server so cached schemas include this required field.

MCP execution tools are `bash` (`command`) and `python` (`code`, Python 3), with `shell` retained as a Bash alias. Both start workspace PTYs and return `terminal_id`, `output`, `exited`, and `exit_code`. `wait_ms` waits up to 10 seconds; use `terminal_read`, `terminal_write`, and `terminal_stop` for ongoing commands. Each Python call starts a fresh interpreter; stdin stays available through `terminal_write`.

Scroll at a screenshot position with this `computer` tool argument object (replace the workspace and target IDs):

```json
{"workspace":"WORKSPACE_ID","summary":"Scroll the selected background window upward to inspect more content.","action":"scroll","window_id":123,"pid":456,"x":800,"y":500,"distance":10,"screen_width":1000,"screen_height":750}
```

`distance: 10` scrolls **up 10 lines**; `distance: -10` scrolls down. Accepted distance is -100 through 100. Legacy `delta` keeps its existing opposite sign (positive down); passing both is an error. Background mode requires `x,y`; desktop mode accepts `x,y` or uses the current pointer if both are omitted. Input actions return an automatic screenshot after the existing two-second settling delay.

To send an image to the MCP server, call `write_image` with a standard image block nested in its JSON arguments:

```json
{"workspace":"WORKSPACE_ID","summary":"Save the supplied image in the workspace and verify its metadata.","path":"images/input.png","image":{"type":"image","mimeType":"image/png","data":"BASE64_ENCODED_IMAGE"}}
```

The tool validates base64, image headers/dimensions, MIME type, extension, size (20 MiB maximum), and workspace path boundaries before writing. Supported formats are PNG, JPEG, GIF, and WebP. The reply includes text metadata and a native MCP `image` content block containing the received image. `read_file` also returns image blocks for these formats, and `computer` returns PNG screenshot blocks. Input images are file uploads; no model vision request is made by `write_image`.

The server negotiates MCP versions 2025-03-26, 2025-06-18, and 2025-11-25. The official Python SDK integration test covers HTTP and stdio:

```sh
uv run --with 'mcp>=1.20,<2' tests/mcp_client.py target/debug/lessagent
uv run --with 'mcp>=1.20,<2' --with pillow tests/computer_background.py target/debug/lessagent
```

The second test requires macOS Accessibility and Screen Recording access and an installed Chrome for its isolated native event fixture. It checks typing, modifier keys, dragging, scroll direction/position, and screenshot images through the MCP SDK; it saves evidence under `output/background-control/`. The test fails if its background Chrome becomes the operating-system foreground app, including after the screenshot delay. Recipient-local document focus is recorded separately, because it is not OS foreground activation. There is no flag to waive a background failure.

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

Verified locally on macOS: Rust tests and lint, backend/CLI integration, actual Codex model discovery using the installed login, and browser mode switching, split terminals, and reload persistence. Direct Codex generation, file editing, and the test-feedback loop were also exercised using the installed login. Audio calls and permission-dependent desktop input have not been exercised. Linux checks are configured in CI; they have not been run on this macOS host.

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

### macOS background computer control

The same `computer` implementation serves MCP (HTTP and stdio), the CLI, normal
agents, and Light mode. Enable computer control in Settings and grant macOS
Accessibility and Screen Recording permissions to the process running Lessagent.

For **Chrome**, first call the `browser_open` tool with an HTTP(S) `url`, or use
`computer` with `action:"browser_open"`. This creates a separate background
Chrome window with an isolated profile and a loopback-only control endpoint.
Use the returned `window_id` and `pid` with the same `computer` tool thereafter:

```json
{"action":"browser_open","url":"http://127.0.0.1:4173/","width":1000,"height":750}
```

Managed Chrome uses browser-local `Input` commands with explicit button state
and a separate virtual pen pointer ID (rather than capturing the human mouse),
not global mouse events or JavaScript-dispatched DOM events. This matters because
Chromium's native macOS event builder samples the physical mouse's button state;
PID-posted native drags can therefore lose the held button during movement.
The user's ordinary browser/profile is never debug-enabled or restarted.
Unmanaged Google Chrome input is rejected with an instruction to use
`browser_open`, rather than falsely claiming reliable isolated input. Window
screenshots and the independent virtual pointer remain native macOS features.
Session records persist in the backend data directory, so managed windows can
be reused after a helper or backend restart. The browser profile is separate
from the user's normal cookies, login sessions, and extensions.

Only page content is controlled through this channel. Browser title bars,
toolbar menus, docked developer tools/side panels, and multiple tabs in the same
managed window are rejected rather than risking the wrong target. Open another
controlled window to navigate independently. Other native applications continue
to use process-directed input and require application-specific verification.

For an existing native application, call `{"action":"windows"}` first, select a window, and send both its `window_id`
and `pid` on every targeted screenshot and input action. Input without a target
is **rejected on macOS**, and explicit `mode:"desktop"` is rejected too. A typo,
missing owner, stale window, or incomplete drag must never move the shared
pointer. There is no macOS global input backend. An initial untargeted
screenshot remains available for read-only desktop discovery.

```json
{"action":"screenshot","mode":"background","window_id":123,"pid":456}
{"action":"click","window_id":123,"pid":456,"x":440,"y":194,"screen_width":2000,"screen_height":1500}
{"action":"type","window_id":123,"pid":456,"text":"Hello from the background"}
{"action":"key","window_id":123,"pid":456,"key":"cmd+a"}
{"action":"drag","window_id":123,"pid":456,"path":[[500,220],[630,618],[291,372],[709,372],[370,618],[500,220]],"duration":3}
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

For non-browser native applications, events use an isolated native source and
are posted once to the specific process/window. They do not warp the system cursor, inject Command into
ordinary clicks, activate the target through LaunchServices, or fall back to a
global HID event stream. A short-lived AppKit responder lease makes the recipient
accept ordinary events without bringing the target to the operating-system foreground.
The recipient may temporarily report `document.hasFocus() == true`; this is not
an OS foreground change. The lease is removed when the action finishes. Click a
text field before typing. Native app menus, protected surfaces, other browser
versions, and multi-window keyboard responders may require separate validation;
posting an event is not a claim that every application accepted it.

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
iteration; direct calls use `agent/output/computer`. MCP returns a native PNG
image content block alongside text metadata. HTTP and normal-agent results keep
binary image data out of JSON text. Input and observation are serialized across
concurrent tool calls. If capture fails after input, `screenshot_error` preserves
that distinction: recapture rather than blindly repeating the input.

Results identify the transport: `delivery:"chrome-devtools"` for managed Chrome,
with `native_input_events_posted:0`, or the native process/window transport for
other applications. They include `input_events_posted`, `responder_lease`,
`before` and `after` desktop state, and the automatic screenshot's `desktop` state. This exposes both immediate and delayed foreground changes.
Human mouse movement can change sampled cursor coordinates; the controller does
not restore/warp over the user's own movement. Windows must be on the current
macOS desktop and not minimized, but can be behind other apps.

Native routing uses runtime-resolved AppKit/Quartz window-event functions.
Unavailable support returns an error instead of silently using desktop input.
Swift is required only at build time; the compiled helper is embedded in release
builds. One serialized helper owns the nonactivating virtual pointer across actions.
Its two one-shot idle timers do not poll focus. It exits when the backend closes
the pipe; a helper failure never replays an action or uses a global input API.

**macOS is background-only.** All mouse, keyboard, drag, and scroll actions
require `window_id` and `pid`; even an explicit `mode:"desktop"` is rejected.
The macOS global-input implementation has been removed, not merely bypassed.
Read-only untargeted screenshots remain available for discovery. Linux X11
desktop input is unchanged.

Run the background-control regression with the official MCP SDK:

```sh
cargo build --release --locked
uv run --with 'mcp>=1.20,<2' --with pillow tests/computer_background.py target/debug/lessagent
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
the real overlay at 9, 10.4, 29, and 30.4 seconds of inactivity. It captures the
actual pointer panel to verify that blue fill becomes transparent but the outline
remains until the 30-second hide. Results are saved in
`output/background-control/report.json`.

Managed Chrome geometry is obtained from the verified browser window, not from
Quartz presentation bounds during Mission Control or Space animations. Returned
PNG dimensions always describe the actual image, including scaled captures;
logical dimensions continue to describe the full window. The isolated Chrome
channel can address its window on another Space when native capture remains
available. Native PID input still requires an onscreen target. No channel falls
back to global input.

Unmanaged Google Chrome windows are read-only through `computer`. Input is
rejected with a `browser_open` instruction. Controlled Chrome uses its isolated
virtual pointer; it never falls back to native input. Native non-Chrome apps
retain process-targeted background input.

Controlled Chrome observations capture the actual page surface, not macOS's
Stage Manager/Mission Control thumbnail. The image preserves window-relative
coordinates by reserving the real browser-toolbar inset as a labeled neutral
band. That band is explicitly **not a captured browser toolbar** and is not
interactive. Metadata identifies `capture_backend: "browser-surface-window-frame"`
and `browser_chrome_captured: false`. This avoids skewed or tiny desktop
representations being misinterpreted as a full-resolution control image. Native
non-Chrome captures use bounded read-only retries; input is never replayed.
