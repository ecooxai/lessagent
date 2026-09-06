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

Keep the backend running. Closing or reloading the browser preserves running jobs and terminals. Stopping the backend stops its live processes; the next start restores workspaces, tabs, drafts, settings, job history, and terminal transcripts. Interrupted jobs are marked interrupted and are not automatically resumed.

## Models and audio

In Settings, choose a provider and use **Discover models**:

| Provider | Authentication | Model |
| --- | --- | --- |
| Codex (default) | Existing `codex login` session | Blank uses the CLI default; discovery uses Codex app-server |
| OpenAI | `OPENAI_API_KEY` or a saved key | Choose a tool-capable Responses API model |
| Claude | `ANTHROPIC_API_KEY` or a saved key | Choose a tool-capable Messages API model |
| Gemini | `GEMINI_API_KEY` or a saved key | Choose a model supporting function calls |

Keys can also be saved in Settings. Environment variables take precedence. Keys are stored locally with owner-only permissions and are never returned to the browser. Existing provider preferences are preserved across upgrades.

Codex requests use the installed CLI with ephemeral, structured responses and its existing authentication. Lessagent executes the returned tool calls. Its planner disables the Codex shell and unified-exec tools and requests a read-only sandbox; other behavior can depend on your Codex configuration. This is a local automation tool, not a security sandbox for untrusted tasks.

Transcription, text-to-speech, and live WebRTC voice use a separate **OpenAI API key**, even when the agent provider is Codex. Audio model IDs are configurable in Settings. Live voice requires microphone permission and an open browser tab. Live voice tools use the workspace selected when the call started.

## Workspaces and context

- **Normal:** reads initial project context, then retains conversation and native tool results during the task. The initial text is capped at 160,000 bytes and 12 images; tools can read other files. Saved user and assistant messages carry between tasks. There is no automatic history compaction.
- **Light:** available only below **30k estimated project tokens** with a complete scan. Before every model call, rebuilds `agent/context/project.txt` containing the file tree and all included text, and sends it with all included images. Previous chat messages are excluded; compact current-task tool observations and saved task knowledge are included.
- Completed tasks write summaries to `agent/knowledge/done/`. Generated context, summaries, and screenshots are excluded from subsequent project scans to avoid recursive growth.
- Click **Project context** in the sidebar for folder and file estimates, sorted by largest token usage. Scans respect `.gitignore`, including nested rules, and skip symlinks. Text estimates conservatively count UTF-8 bytes; image estimates use image dimensions. These are model-independent admission estimates, not billing token counts.
- Files larger than 16 MiB and bundles larger than 64 MiB prevent Light mode. Unsupported binary files are not model context; audio and video are available as previews. Ensure sensitive files are ignored before sending project context to a provider.

The sidebar shows folder names; hover for the full path, or double-click a folder to choose Normal or Light mode. The full workspace path appears above the tabs. Use **＋** beside the tabs to open Files, Terminal, Managed terminals, or Webview. Managed terminals are started by the agent and list running commands first, then idle sessions, with newer sessions first within each group. Tabs support an agent view, terminal grid, sandboxed webviews, settings, logs, text editing, and image/audio/video previews. Terminal cards have a 300px minimum width, split into additional sessions, resize vertically, and preserve their height. Sites that prohibit embedding can be opened in a separate browser tab.

All shell tools run in real PTYs maintained in backend memory. Output is also persisted, with a 256 KiB tail per terminal. Tool reads return the newest 24,000 bytes. There can be up to 32 live terminals. **Stop agent** cancels the model loop; use a terminal's **Stop** button to stop a command that is still running.

## Computer control

Enable mouse, keyboard, and screenshots in Settings.

- **macOS:** uses CoreGraphics and `screencapture`. Grant Accessibility and Screen Recording permission to the terminal/application hosting the backend.
- **Linux X11:** install `xdotool` and `scrot`, and run the backend in the desktop session.
- **Linux Wayland:** `grim` supports screenshots; mouse and keyboard injection currently require X11.

Supported actions include screenshot, move, click, drag, type, key, and scroll. Commands execute with the backend user's operating-system permissions. File tools enforce workspace path boundaries; Bash and computer control intentionally have broader host access.

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

Verified locally on macOS: Rust tests and lint, backend/CLI integration, actual Codex model discovery using the installed login, and browser mode switching, split terminals, and reload persistence. Live paid-provider generation, audio calls, and permission-dependent desktop input have not been exercised. Linux checks are configured in CI; they have not been run on this macOS host.

The implementation uses twelve direct Rust dependencies and plain HTML/CSS/JavaScript. `src/agent.rs` owns the agent loop, `provider.rs` provider translation, `context.rs` inventory/bundling, `terminal.rs` PTYs, `computer.rs` platform input, and `server.rs`/`mcp.rs` the interfaces. Release builds enable thin LTO and strip symbols. No performance benchmark has been run.

Context ignore patterns are editable in Settings, with visible defaults and a reset button. Defaults exclude lockfiles, licenses, temporary files and common generated directories. These rules apply to context scans and agent bundles, while the Files browser still displays ignored files. Token estimates use k units.

Files, Context and the workspace picker share a tree browser. The workspace picker starts with regular folders and provides a hidden-folder toggle. Its path input accepts absolute paths, `~/` paths, or paths relative to the current folder; typing offers fuzzy matches for folder names within the typed parent directory. Press Enter to navigate to a path, or choose a suggestion. Opened workspaces and the selected workspace are saved and restored across backend restarts. Expand folders with a click, enter with a double-click, and use the parent arrow to go up. Open folders refresh every three seconds; newly discovered files appear first in blue for 30 seconds. The Files terminal button opens a folder terminal dock; navigating reuses an idle shell or creates a new one if the selected shell is busy.

The terminal uses a custom JavaScript screen renderer backed by the in-memory VT100 parser, without a frontend terminal dependency. It supports ANSI colors, direct keyboard input, paste, IME input, resize, scrollback and mobile key controls. Selections longer than four characters are copied after three seconds when browser clipboard permissions allow; an explicit Copy button is available as well.
