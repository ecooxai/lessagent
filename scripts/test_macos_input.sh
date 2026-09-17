#!/usr/bin/env bash
# Serial, real-GUI debug regression. Never restarts the connected MCP backend.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin ]] || { echo 'This regression requires macOS.' >&2; exit 2; }
command -v uv >/dev/null || { echo 'uv is required for the MCP SDK test environment.' >&2; exit 2; }
[[ -d /Applications/Google\ Chrome.app && -d /Applications/Blender.app ]] || {
  echo 'Install Chrome and Blender before running this GUI regression.' >&2; exit 2;
}
REPORT="output/macos-input-suite/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$REPORT"
printf 'Tests briefly foreground only disposable app windows. Keep input idle for strict isolation checks.\nReports: %s\n' "$REPORT"
cargo build --locked
# Includes the deterministic background Chrome helper regression on local macOS.
cargo test --locked
uv run --with 'mcp>=1.20,<2' tests/macos_app_input.py target/debug/lessagent \
  --apps chrome --report-dir "$REPORT/chrome-native"
uv run --with 'mcp>=1.20,<2' tests/macos_app_input.py target/debug/lessagent \
  --apps chrome --chrome-devtools --report-dir "$REPORT/chrome-devtools"
uv run --with 'mcp>=1.20,<2' tests/macos_app_input.py target/debug/lessagent \
  --apps blender --report-dir "$REPORT/blender"
printf '\nPASS: all supported-action checks and explicit no-input rejections. Reports: %s\n' "$REPORT"
