# MCP guidance and per-call summary validation

Implemented in `/Users/ecoo/project/agent/lessagent` on September 9, 2026.

## Changes

- Initialization, all 16 MCP descriptions, and workspace open/list responses instruct clients to read Agents.md first (AGENTS.md fallback, full pagination, nested guidance).
- Every MCP tool requires a nonblank string summary of at most 1000 Unicode characters. Validation happens before dispatch; summary is removed from execution arguments and returned as client-intent text plus structuredContent.summary.
- structuredContent.result, tool errors, file pagination, images, PTY output and internal agent/CLI APIs retain their existing contracts.
- Added Agents.md with project layout, debug/test commands, isolated ports/data, reproducible jsdom setup and separate production release commands. README and SDK/native test clients were updated.

## Final results

| Check | Result |
| --- | --- |
| cargo fmt --check | PASS |
| cargo clippy --locked --all-targets -- -D warnings | PASS |
| cargo test --locked | 47 passed |
| cargo build --locked | PASS; dev profile, unoptimized + debuginfo |
| MCP SDK 1.20.0 and 1.30.0 | HTTP and stdio passed for every tool |
| Backend smoke, startup/TUI, legacy migration | All passed with target/debug/lessagent |
| JavaScript regressions, jsdom 30.0.0 | 9/9 passed; node --check also passed |
| Native background-control retry | PASS: 44 actions, 14 check groups, 555 accuracy samples |
| git diff --check | PASS |

See final-checks.json, backend-tests.json, mcp-sdk-1.20.log, mcp-sdk-1.30.log and background-control-retry/report.json for evidence. Source hashes and the complete check commands are in summary.json; patch files compare only this task's changes against the pre-edit snapshots rather than against HEAD (the checkout already contained user changes).

## Initial failures and scope

The first temporary JavaScript environment used jsdom 26.1.0, which lacked onpointerdown. Upgrading only the temporary dependency to jsdom 30.0.0 made the unchanged thinking-menu test pass, followed by all nine UI tests. Initial results remain in javascript-tests-jsdom26-initial.json.

The first native run failed a foreground-focus assertion; a physical-click counter increased during that action. The full test passed unchanged on retry. No safety assertion was relaxed, and the first failure is retained in background-control/report.json and computer-background.log. Pillow emitted nonfatal deprecation warnings in the native suite.

All executed development and integration tests used debug builds and separate ports/data directories. No release build/deployment, Linux run, or paid model call was performed. The pre-existing running backend was not restarted. Clients must refresh tool discovery after an explicitly requested deployment of the new binary.
