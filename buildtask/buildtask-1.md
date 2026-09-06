build an general ai agent for user to control computer
the agent should be able to control mouse keyboard,get screenshoot, run command bash.
support main AI api like openai api, gemini api, claude api 
also suppport codex auth to use openai model via codex,get all available model ,support audio model and gpt-live audio api
also support powered by mcp server so other ai chat like chatgpt can connect it and use this agent tool
support workspace at left as vertical bar, each workspace can open a folder to work
at bottom of workspace, show current project size in tokens use k as unit, get all text files total tokens,image files total tokens, respect .gitignore
at topbar, user can use tabs like webview, terminal(for show virtual terminals in one tab ,each with 300px, show more in one row if width is big, scrollable to see more terminals or create new terminal, support split terminal), settings, logs, open files like image ,text,audio, video
there 2 mode, 
the light mode that send whole project text files images, put all code files in one  text file when send, show file structure in this one file, send this file and images every request, do not send previous msg, save previous msg summary in each workspace folder ./agent/knowledge/done
the normal mode like codex, get all context first and continous work
only use light mode when total project tokens <30k

support linux , macos, use Rust language, ensure build and running speed, high performance
use less depency libs, try to implement common function in rust directly.
support cli and ui in browser, run virtual ui in ram and map to ui when need, save all ui status in bg as backend so even ui is close it still run, the backend should be http server host webui and communicate by http
also support virtaul terminals, when run bash cmd, run cmd in the virtual terminal, user can see these virtual terminal in ui too

fast and minimal style in ui, ensure high performance and clever agent

## Implementation status — 2026-09-06

- [x] Rust backend with embedded browser UI, CLI, and persistent background jobs.
- [x] OpenAI, Gemini, Claude, and Codex provider adapters with model discovery. Codex CLI authentication is the default.
- [x] Workspace sidebar, project inventory respecting .gitignore, separate text/image/total estimates in k, and strict Light mode admission below 30k.
- [x] Light mode rebuilds the combined text/tree file and images for every call, excludes previous chat, and uses completed-task summaries in agent/knowledge/done. Normal mode retains conversation and tool context.
- [x] Tabs for agent, terminal grid, webviews, settings, logs, text, images, audio, and video.
- [x] Light top bar in Light mode and dark top bar in Normal mode.
- [x] Visible Bash PTYs, split/new/resize/stop, 300px minimum cards, backend screen state, persistent transcripts, tabs, drafts, and terminal heights.
- [x] macOS computer control; Linux X11 input and X11/Wayland screenshots.
- [x] OpenAI transcription, speech, and browser WebRTC live voice with tool calls.
- [x] Authenticated MCP HTTP POST and a stdio bridge for local MCP clients.
- [x] Setup documentation, MIT license, Rust unit tests, backend/CLI integration tests, and macOS/Linux CI configuration.

Validation and limits: See ../README.md. Local macOS checks cover build, lint, tests, real Codex model discovery, and browser interactions. Paid API calls, live voice, desktop input permissions, and Linux execution still require environment-specific validation. Native Wayland input and direct cloud access to the loopback MCP server are not implemented. Backend restart restores saved state and transcripts, not live operating-system processes. Token counts are conservative estimates; Normal mode initial context is bounded and history is not automatically compacted.
