# Lessagent project guidance

## Read first and working rules

Read this entire file before inspecting implementation files, running commands, editing, or delegating work. When working below the repository root, read applicable nested `Agents.md` / `AGENTS.md` files as well. Use the `newlessagentneo` MCP tools on the Mac; the default checkout is `/Users/ecoo/project/agent/lessagent`. Open the workspace once and reuse its returned ID. Fall back to another execution environment only if the MCP host tools do not work, and report that limitation.

Preserve unrelated tracked and untracked work. Check `git status --short` before editing; do not reset, clean, stash, commit, or restart the user's running backend without an explicit request. Keep changes scoped and inspect existing files before replacing them. Shell tools run on the host, not in a sandbox. Do not expose secrets, tokens, or personal data in commands, summaries, logs, or reports.

Use **debug builds for all development, debugging, and tests**. `cargo build --locked` and `cargo run --locked` use Cargo's dev profile and produce `target/debug/lessagent`; `cargo test --locked` uses the unoptimized test profile. `build.rs` also compiles the Swift helper with `-Onone -g` outside the release profile. There is no standard `cargo debug` command. Do not use `--release` for development or tests. Release commands below are for an explicitly requested production build/run only.

Always start development/test servers with a **different `--port` from the normal backend (default 3210)** and a disposable `--data-dir`. Never run test `stop` / `restart` against the default port or the user's saved state. Prefer the foreground `serve` command and stop only processes created for the test. A running server is verified with an HTTP request; do not poll its terminal waiting for it to exit.

## Project summary and structure

Lessagent is a persistent local computer and coding agent implemented in Rust. One backend serves a browser UI, CLI/TUI clients, and MCP over HTTP or a stdio bridge. It owns workspaces, saved settings/history, model-provider requests, agent jobs, and real PTY terminals. Browser assets are embedded; there is no required frontend build step.

| Location | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Rust package, locked dependencies, release profile. |
| `src/main.rs` | CLI parsing, startup/reconnection, stop/restart, MCP stdio bridge. |
| `src/lib.rs` | Module wiring and shared helpers. |
| `src/server.rs` | HTTP routes, browser API, authentication, backend lifecycle. |
| `src/mcp.rs` | MCP initialization, resource/tool dispatch, schemas, annotations, summaries and presentation. |
| `src/resources.rs`, `docs/instruction.md` | Server-owned instruction.md resource, live host metadata, resource/tool bridges. |
| `src/image_content.rs` | Encoded image dimensions and shared MCP image metadata. |
| `src/tools.rs` | Shared agent/CLI tool definitions and execution: shell/Python, files, terminals, computer. |
| `src/agent.rs` | Agent loop, sessions, verification, compaction, continuity and artifacts. |
| `src/provider.rs` | Model discovery, authentication, provider request/response translation. |
| `src/state.rs` | Persistent state, workspace/job records and event archives. |
| `src/context.rs`, `src/git.rs` | Context inventory/ignore rules, file boundaries, repository operations. |
| `src/terminal.rs`, `src/tui.rs` | PTY lifecycle, VT100 terminal state and terminal UI. |
| `src/computer.rs` | Platform computer-control integration. |
| `native/computer.swift`, `native/browser.swift`, `native/system.swift`, `build.rs` | macOS native helper and isolated Chrome control, compiled/embedded by Cargo. |
| `web/` | Plain HTML/CSS/JavaScript UI embedded in the binary. |
| `tests/` | Python backend/CLI/MCP integration, JavaScript UI regressions and native-control fixtures. |
| `.github/workflows/` | macOS/Linux CI checks. |
| `agent/output/` | Real task deliverables and image evidence, not bookkeeping. |
| `agent/continuity/`, `agent/output-history/` | Internal continuity records and older artifact snapshots. |
| `target/` | Cargo build intermediates; use the debug profile for development/tests. |
| `output/` | Important finished deliverables: verified binaries, images, 3D models, renders and documents; keep validation evidence in named subfolders. |

Runtime data lives outside the checkout by default, under `$XDG_DATA_HOME/lessagent` or `~/.local/share/lessagent`. `--data-dir` / `LESSAGENT_DATA_DIR` overrides it; `--port` / `LESSAGENT_PORT` selects the server port. All clients, including the MCP stdio bridge, must match the intended backend settings.

## Debug build and run commands

Run these from the repository root. Requirements include a Rust toolchain compatible with the checkout (README specifies Rust 1.95+), Bash, Python 3, and macOS Xcode command-line tools for the Swift helper. Node is needed for JavaScript checks; `uv` supplies the optional Python MCP SDK test environment.

```sh
cargo build --locked

# Foreground debug backend: use another free port if 33210 is occupied.
DEV_DATA="$(mktemp -d "${TMPDIR:-/tmp}/lessagent-dev.XXXXXX")"
./target/debug/lessagent serve --port 33210 --data-dir "$DEV_DATA"
# Ctrl-C stops this foreground test backend, not the normal backend.
```

For an app-style debug launch, plain `cargo run` runs the default Lessagent launch mode: it starts or connects to the backend service and opens the standalone native service-monitor window. During development, still pass an isolated port/data directory after `--`:

```sh
cargo run --locked -- --port 33210 --data-dir "$DEV_DATA"
```

For a headless foreground backend instead, use:

```sh
cargo run --locked -- serve --port 33210 --data-dir "$DEV_DATA"
```

From another terminal, verify `http://127.0.0.1:33210/health`. For a stdio MCP client connected to that debug server, pass the exact same data directory:

```sh
./target/debug/lessagent mcp --port 33210 --data-dir /absolute/path/to/the/disposable-data-dir
```

Changing MCP code does not update an already running binary. Do not restart the MCP backend serving your own working connection during development. Verify the new debug binary on an isolated port. After an explicitly requested deployment, reconnect clients or refresh `tools/list` so cached schemas are replaced.

## Validation commands

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

The Python integration harnesses above choose isolated ports/data directories. `tests/mcp_client.py` covers HTTP and stdio with the real MCP SDK, all public tool schemas, invalid summaries, preserved file/image/PTY results, read-first guidance, removed-tool rejection, and resource behavior. It does not need real model credentials or make paid model calls. The smoke harness uses its own local provider fixture.

JavaScript regression scripts in `tests/*.cjs` need a `jsdom` version with pointer-event handler support. This isolated setup was tested with Node 22.23.1 and `jsdom` 30.0.0; an older `jsdom` 26 environment cannot dispatch the thinking-menu test's `onpointerdown` handler. Install test dependencies outside the checkout so no project manifest or lockfile is changed:

```sh
JS_TEST_DEPS="$(mktemp -d "${TMPDIR:-/tmp}/lessagent-js-tests.XXXXXX")"
npm install --prefix "$JS_TEST_DEPS" --no-package-lock --ignore-scripts jsdom@30.0.0
for test in tests/*.cjs; do
  NODE_PATH="$JS_TEST_DEPS/node_modules" node "$test" || exit 1
done
```

For computer-control changes, also run:

```sh
# On a local macOS development machine, cargo test automatically runs the
# deterministic managed-Chrome pointer regression. CI/headless runs skip
# that GUI/TCC test; LESSAGENT_SKIP_GUI_TESTS=1 is the explicit local escape hatch.
cargo test --locked
uv run --with 'mcp>=1.20,<2' tests/browser_profile_virtual_tools.py target/debug/lessagent
uv run --with 'mcp>=1.20,<2' --with pillow tests/native_app_virtual_tools.py target/debug/lessagent
# Launch/reuse only: owned AppKit fixture, no personal apps or input.
uv run --with 'mcp>=1.20,<2' tests/app_open_mcp.py target/debug/lessagent
# Real mesh operations + Chrome circle, also in the local cargo test integration:
python3 tests/background_modeling.py target/debug/lessagent
```

`browser_open` and `app_open` are restored public MCP tools. Use `browser_open` for a new controlled Chrome window with the existing persistent Lessagent-managed profile; `new_profile:true` explicitly requests a separate blank managed profile. Personal Chrome profile data is never copied or changed. Use `app_open` for native apps, including Blender's `--no-window-focus` launch. The restored launcher defaults to a new instance; `new_instance:false` reuses one unambiguous existing window. Reuse exact returned window IDs/PIDs and screenshot dimensions for `virtual_pointer` / `virtual_keyboard`. Do not substitute raw shell launching or a native Chrome input fallback. See `docs/instruction.md`.

## MCP contract

Read the server resource `lessagent://server/instruction.md` through standard MCP `resources/list` / `resources/read`, or the read-only `list_resources` / `read_resource` tools for tool-only clients. These bootstrap operations do not require a workspace or computer control enabled. The resource includes freshly read OS, CPU, GPU, RAM, all macOS displays (logical/backing-pixel/visible sizes), and workflow rules. It excludes credentials, serial numbers, user/host names and window contents. Refresh it and `tools/list` after display changes. Native resource operations do not take summary; the two tool bridges do.

Use standalone `list_windows`, `get_screenshot`, `virtual_pointer`, and `virtual_keyboard` for MCP GUI work; the aggregate `computer` MCP tool is removed. MCP still does not expose `agent_run`, `agent_status`, `read_file`, or `write_file`; the client orchestrates work using Bash/Python and the restored GUI tools. Prefer an app's normal/default profile or session: reuse an existing window when available, otherwise launch it with app_open, and create a new instance/profile only when the user explicitly asks. Native events use exact Cocoa window IDs and AX geometry. Native-app input uses the pre-removal process/window route without a responder lease; Chrome uses its registered managed DevTools target. Ordinary left-clicks on actionable Accessibility controls use exact-window `AXPress` when available; canvas/viewport clicks and other pointer actions retain the recipient-local process event path, and one user action is never double-sent. Verify both foreground PID and target window presentation: Stage Manager can expand a window without changing foreground PID. Native captures may be explicitly marked, perspective-corrected low-resolution thumbnails. Never claim full-resolution capture from dimensions alone or treat successful posting as proof an app accepted input. Unsupported application behavior does not justify global input or activation fallbacks. See `docs/background-control.md`.

Image outputs carry byte-verified `width`, `height`, `format`, and `image_metadata`; screenshots also retain `screen_width` / `screen_height`. MCP image blocks expose `_meta["lessagent/image"]` and compatibility dimension aliases. Pass actual screenshot dimensions with pixel coordinates. A client-specific `fovea` or inspector field is not a standard MCP requirement and may still be omitted by its adapter. No fictitious crop is emitted.

Direct screenshots now default to project `output/computer/`. Preserve internal agent iteration/continuity locations but copy important finished binaries, images, 3D files and other deliverables to project `output/`. Keep source code in its source directories and do not overwrite unrelated output. End each completed task with a factual summary of changes, artifact paths, tests/results and remaining limitations. Changing source does not deploy the running backend.

Initialization, tool descriptions, and successful workspace open/list responses instruct clients to read workspace-root `Agents.md` first with Bash/Python. Try `AGENTS.md` when absent; if neither exists, report that and proceed. Read applicable nested guidance before subdirectory edits. This is workflow guidance, not a server-side assertion that a client obeyed it.

Every MCP tool, including `workspace_open`, `workspace_list`, `list_resources`, and `read_resource`, requires eight request-only observability fields: `summary`, `agent`, `model`, `main_task`, `current_task`, `progress`, `quality`, and `current_timestamp`. `summary` is a 1–500 Unicode-character observability paragraph: state verified work completed so far, include any relevant mistake or correction if one occurred, then say what the current tool call will do and why. Never invent mistakes, claim unverified success, or repeat main task, current task, progress, or quality inside it. `main_task` is the overall task name, `current_task` is the current step, and `progress`/`quality` are integer 0–100 scores. `agent`, `model`, and `current_timestamp` retain their caller-identification/timestamp meanings. Example: `{"summary":"Verified the MCP schema and found the previous 49-character cap made summaries too terse. No execution error occurred; this call updates the schema/tests to allow 500 characters and preserve richer progress context.","agent":"ChatGPT","model":"GPT-5.6 Sol","main_task":"Improve MCP summaries","current_task":"Patch summary contract","progress":60,"quality":92,"current_timestamp":"2026-09-12T08:30:00-07:00"}`. Detailed metadata guidance lives in `instruction.md`, not repeated in every tool description. The adapter validates and strips these fields before execution, while Logs records them with sanitized arguments/output. This requirement is MCP-only; internal agent tools and direct CLI/HTTP tool APIs keep their existing contracts.

## Release build and production run (not development/test)

Only use these when a release build or production deployment is explicitly requested:

```sh
cargo build --release --locked
./target/release/lessagent
# Or a foreground production server with deliberate settings:
./target/release/lessagent serve --port 3210 --data-dir "$HOME/.local/share/lessagent"
```

Check existing listeners and data-directory ownership before starting production. Do not replace or stop an active backend as part of ordinary development. Release validation/deployment requires a separate explicit request; the normal development verification above stays on debug builds.
