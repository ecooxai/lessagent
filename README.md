# Lessagent

A persistent local computer and coding agent written in Rust, with a browser UI, CLI, and MCP server. Codex authentication is the default provider. Light mode uses a pale top bar; Normal mode uses a dark top bar.

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

State uses atomic file replacement, owner-only credential files, and a single-server lock per data directory. The HTTP server listens on all IPv4 interfaces (`0.0.0.0`), accepts requests using the server IP address or hostname, validates browser Origin, and rejects cross-site browser requests. Open `http://SERVER_IP:3210/` from another device; remote access requires a password configured with `lessagent --passwd PASSWORD`, or a valid token. Loopback browser API access needs no token unless a password is configured. MCP and server administration always require the bearer token. Optional URL fragment tokens are moved into browser session storage and removed from the address bar.

## MCP

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

Clients supporting stateless MCP HTTP POST can use `http://127.0.0.1:3210/mcp` with `Authorization: Bearer <token from data-dir/token>`. JSON-RPC notifications return HTTP 202. The implementation supports initialization, ping, tool discovery, and calls; it does not provide an SSE event stream. A cloud-hosted chat such as ChatGPT cannot reach your loopback address directly; connecting it requires a compatible trusted bridge and authentication outside this project's local server.

## Development and validation

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/smoke.py
node --check web/app.js
```

`python3 tests/legacy_startup.py target/release/lessagent` verifies migration from an older backend without administration endpoints. `tests/workspace_picker.cjs` exercises the picker DOM with jsdom; run it with `node` in an environment where jsdom is installed.

`python3 tests/startup.py` verifies automatic startup, app/MCP port logs, the startup menu, reconnection, TUI workspace opening and scrolling, detachment, stop/restart, token-free local access, password login and changes, password persistence, and MCP authentication.

The smoke test starts an isolated backend and a deterministic Codex fixture. It checks authentication and origin boundaries, project context, model discovery, tool execution, cancellation, terminal survival without a browser, MCP over HTTP/stdio, and persistence after restart. It makes no paid API calls. Pass a binary path to test a release build: `python3 tests/smoke.py target/release/lessagent`.

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
