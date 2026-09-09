# Lessagent project guidance

## Read first and working rules

Read this entire file before inspecting implementation files, running commands, editing, or delegating work. When working below the repository root, read applicable nested `Agents.md` / `AGENTS.md` files as well. Use the `newlessagentneo` MCP tools on the Mac; the default checkout is `/Users/ecoo/project/agent/lessagent`. Open the workspace once and reuse its returned ID. Fall back to another execution environment only if the MCP host tools do not work, and report that limitation.

Preserve unrelated tracked and untracked work. Check `git status --short` before editing; do not reset, clean, stash, commit, or restart the user's running backend without an explicit request. Keep changes scoped and inspect existing files before replacing them. Shell tools run on the host, not in a sandbox. Do not expose secrets, tokens, or personal data in commands, summaries, logs, or reports.

Use **debug builds for all development, debugging, and tests**. `cargo build --locked` and `cargo run --locked` use Cargo's dev profile and produce `target/debug/lessagent`; `cargo test --locked` uses the unoptimized test profile. There is no standard `cargo debug` command. Do not use `--release` for development or tests. Release commands below are for an explicitly requested production build/run only.

Always start development/test servers with a **different `--port` from the normal backend (default 3210)** and a disposable `--data-dir`. Never run test `stop` / `restart` against the default port or the user's saved state. Prefer the foreground `serve` command and stop only processes created for the test. A running server is verified with an HTTP request; do not poll its terminal waiting for it to exit.

## Project summary and structure

Lessagent is a persistent local computer and coding agent implemented in Rust. One backend serves a browser UI, CLI/TUI clients, and MCP over HTTP or a stdio bridge. It owns workspaces, saved settings/history, model-provider requests, agent jobs, and real PTY terminals. Browser assets are embedded; there is no required frontend build step.

| Location | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Rust package, locked dependencies, release profile. |
| `src/main.rs` | CLI parsing, startup/reconnection, stop/restart, MCP stdio bridge. |
| `src/lib.rs` | Module wiring and shared helpers. |
| `src/server.rs` | HTTP routes, browser API, authentication, backend lifecycle. |
| `src/mcp.rs` | MCP initialization, tool schemas/descriptions, summary validation, dispatch adapter and text/image results. |
| `src/tools.rs` | Shared agent/CLI tool definitions and execution: shell/Python, files, terminals, computer. |
| `src/agent.rs` | Agent loop, sessions, verification, compaction, continuity and artifacts. |
| `src/provider.rs` | Model discovery, authentication, provider request/response translation. |
| `src/state.rs` | Persistent state, workspace/job records and event archives. |
| `src/context.rs`, `src/git.rs` | Context inventory/ignore rules, file boundaries, repository operations. |
| `src/terminal.rs`, `src/tui.rs` | PTY lifecycle, VT100 terminal state and terminal UI. |
| `src/computer.rs` | Platform computer-control integration. |
| `native/computer.swift`, `native/browser.swift`, `build.rs` | macOS native helper and isolated Chrome control, compiled/embedded by Cargo. |
| `web/` | Plain HTML/CSS/JavaScript UI embedded in the binary. |
| `tests/` | Python backend/CLI/MCP integration, JavaScript UI regressions and native-control fixtures. |
| `.github/workflows/` | macOS/Linux CI checks. |
| `agent/output/` | Real task deliverables and image evidence, not bookkeeping. |
| `agent/continuity/`, `agent/output-history/` | Internal continuity records and older artifact snapshots. |
| `target/`, `output/` | Cargo build outputs and test reports; not source. |

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

The equivalent build-and-run command is:

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

The Python integration harnesses above choose isolated ports/data directories. `tests/mcp_client.py` covers HTTP and stdio with the real MCP SDK, all tool schemas, invalid summaries, preserved file/image/PTY results, read-first guidance, and agent run/status against a deterministic local provider fixture. It does not need real model credentials or make paid model calls. The smoke harness also uses a local fixture.

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
uv run --with 'mcp>=1.20,<2' --with pillow tests/computer_background.py target/debug/lessagent
```

That integration test needs Chrome plus macOS Accessibility and Screen Recording permissions. Keep the shared pointer and user's normal browser/profile untouched. Missing permissions or unavailable dependencies must be reported as blockers, not passes. Do not weaken assertions or use placeholder verification. Report exact commands, results, and any untested behavior; retain useful failure evidence separately from source.

## MCP contract

Initialization, tool descriptions, and successful workspace open/list responses instruct clients to read workspace-root `Agents.md` first with `read_file`. Try `AGENTS.md` when the first spelling is absent; if neither exists, report that and proceed. Follow `has_more` / `next_offset` until guidance is fully read. This is client workflow guidance, not a server-side assertion that a client actually read or obeyed the file.

Every MCP tool, including `workspace_open`, `workspace_list`, `agent_run`, and `agent_status`, requires a `summary` string: a concise, nonblank paragraph describing the specific call's intended action and purpose, at most 1000 Unicode characters. Do not claim an outcome before observing the result. For example:

```json
{"workspace":"WORKSPACE_ID","path":"Agents.md","summary":"Read the project guidance before inspecting code or running tests."}
```

The MCP adapter validates summaries before side effects, removes the metadata before dispatch, and returns the trimmed summary as a labeled text paragraph and `structuredContent.summary`. Actual execution data remains under `structuredContent.result`; `isError` remains authoritative for tool errors. Invalid summaries produce a repairable tool error. Valid summaries are also returned when execution fails and must not be mistaken for evidence of success. Shell commands, Python code, and delegated prompts never execute summary text. This requirement is MCP-only: internal agent tools and direct CLI/HTTP tool APIs keep their existing contracts.

## Release build and production run (not development/test)

Only use these when a release build or production deployment is explicitly requested:

```sh
cargo build --release --locked
./target/release/lessagent
# Or a foreground production server with deliberate settings:
./target/release/lessagent serve --port 3210 --data-dir "$HOME/.local/share/lessagent"
```

Check existing listeners and data-directory ownership before starting production. Do not replace or stop an active backend as part of ordinary development. Release validation/deployment requires a separate explicit request; the normal development verification above stays on debug builds.
