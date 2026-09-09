# Lessagent MCP instructions

## Read this resource first

This is `lessagent://server/instruction.md`, a server resource, not a path to open on the client machine. Discover it with MCP `resources/list` and read it with `resources/read` using its exact URI. Tool-only clients can call Lessagent's `list_resources` and `read_resource` tools instead; both are read-only and require a per-call `summary`. No workspace or computer permission is needed to read this guide. Re-read it and refresh `tools/list` after changing displays or screen resolution; live OS, CPU, GPU, RAM and display information is appended below. Resource subscriptions are not supported.

Open the requested absolute project folder with `workspace_open`, or recover its ID with `workspace_list`. Before project work, read the entire workspace-root `Agents.md` using `read_file`; try `AGENTS.md` if absent and check applicable nested guidance before editing a subdirectory. Follow `has_more` and `next_offset` until guidance is complete. Report missing guidance rather than assuming it exists. Discovery and reading guidance are allowed bootstrap steps.

## Coding workflow

Every tool call must include a concise, nonblank `summary` paragraph of at most 1000 Unicode characters explaining what this particular call will do and why. Summaries describe intent, not unverified success. They are metadata, never shell code, Python code, GUI input, or a delegated prompt. Do not put secrets into summaries.

Inspect relevant files and the working tree, preserve unrelated work, make scoped edits, then run meaningful checks. Use `bash` or its `shell` alias for build/test commands and `python` for scripts; these execute on the host with its user's permissions, not in a sandbox. File tools enforce workspace boundaries. Read existing files before replacing them. Scope searches to relevant project folders. `read_file` defaults to 4000 bytes of text and caps each call at 8000; paginate rather than dumping entire large files.

Use debug builds for development and testing (`cargo build --locked`, `cargo test --locked`, `target/debug/lessagent` for this Rust project). There is no standard `cargo debug` command. Reserve release builds for explicitly requested production work. Start test services on a different, free `--port` and with a disposable `--data-dir`, never the normal backend's port/state. Verify running services over HTTP instead of waiting for them to exit. Do not stop or replace the backend serving your current MCP connection.

Commands return `terminal_id`, output and exit status. Use `terminal_read` for bounded follow-up, `terminal_write` for intentional input, and `terminal_stop` only for your own processes. Stop polling when a process exits; do not treat a still-running process as a successful check. Use `agent_run` only to delegate an authorized task, then inspect `agent_status`; a started job is not a completed job.

## Computer and browser control

Prefer file/command tools for coding; use `computer` when GUI interaction is needed. Computer actions require computer control enabled in Settings. On macOS, screenshots require Screen Recording; native app input requires Accessibility. Missing permissions are errors, not a reason to use global input.

For Chrome, call `browser_open` (the tool name, not `open_browser`), or `computer` with `action:"browser_open"`, with an HTTP(S) URL. Omit dimensions for a new **1000 by 600 logical-point window** by default. Width/height maxima follow the current **primary display's logical resolution**, not its Retina pixel resolution; launch-time validation rejects larger explicit requests even with stale client schemas. The window is fitted and centered within that display's visible work area, excluding the menu bar and Dock. If the work area cannot fit the minimum 640 by 480 window, opening fails. Default dimensions are reduced on smaller displays; the result's `browser_size` reports requested, actual, maximum and visible sizes. Missing display information is reported, never fabricated.

Each `browser_open` creates an isolated background Chrome window/profile. Never debug-enable, restart, or commandeer the user's normal browser. Reuse the returned `window_id` and `pid` for subsequent `computer` actions. The created window includes an automatic screenshot. Only page content is controllable; browser toolbars/title bars and multiple tabs in one managed window are not supported. Open another controlled window for independent navigation. Unmanaged Chrome input is rejected; do not work around this through native events or the system pointer.

For another native app, call `computer` with `action:"windows"`, choose the intended `window_id` and `pid`, then screenshot that same target. On macOS all input is background-only and never moves the shared physical pointer or deliberately activates the app. An unsupported target must fail without a desktop/global-input fallback. Linux X11 supports desktop input; Wayland supports screenshots only. Isolated Chrome control described above is macOS-only.

Use the returned **image's actual pixel `width`/`height`**, also available as `screen_width`/`screen_height`, as the coordinate reference. Pass those dimensions as `screen_width` and `screen_height` with pixel coordinates for the same screenshot. Logical window sizes, display resolution and screenshot pixels are different quantities; never substitute one for another. Image metadata is in `structuredContent.result.image_metadata` and the MCP image block's `_meta["lessagent/image"]`; dimension aliases are also emitted for compatible adapters. The PNG/JPEG/GIF/WebP bytes are authoritative. These are full-frame images, not foveated crops; no invented `fovea` is supplied. A client's private image inspector may ignore extension metadata.

Every successful input waits for a fresh automatic screenshot; inspect it before choosing the next action instead of taking redundant screenshots. If `screenshot_error` is present, the input may already have succeeded: recover with a read-only screenshot, never replay the input blindly. Scroll uses `distance` (positive up, negative down); do not combine it with legacy `delta`. Drag can use a continuous `path` and screenshot dimensions. Do not weaken safety checks or mistake virtual document focus for an OS foreground change.

## Deliverables and completion

Save important finished work in the active **project directory's `output/` folder**: compiled binaries, images, screenshots, 3D models/scenes, renders, documents, and other usable deliverables. Keep source edits in their normal project locations and Cargo intermediates in `target/`; copy the final verified binary to `output/` when it is a deliverable. Do not move, delete, or overwrite unrelated output. Use descriptive task subfolders to avoid collisions. Direct computer screenshots default to `output/computer/`; `capture_path` can choose another workspace-relative output location.

Internal agent continuity and intermediate iteration artifacts may still use `agent/continuity/` and `agent/output/`. Copy important finished artifacts to project `output/` rather than leaving the only copy in temporary/session directories. Keep logs and test evidence separate from finished artifacts, for example `output/<task>/validation/`.

After finishing a task, give a factual **final summary**: what changed, where the finished outputs are, which checks ran and their actual results, and any failures or untested limitations. Distinguish implemented/tested work from deployment; never claim that changing source updated an already running backend. Do not claim success solely from the client-provided call summary.
