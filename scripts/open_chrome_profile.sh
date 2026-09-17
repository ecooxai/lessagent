#!/usr/bin/env bash
set -euo pipefail

CHROME_APP="${CHROME_APP:-/Applications/Google Chrome.app}"
CHROME_BIN="${CHROME_BIN:-$CHROME_APP/Contents/MacOS/Google Chrome}"
CHROME_ROOT="${CHROME_ROOT:-$HOME/Library/Application Support/Google/Chrome}"
URL=""
PROFILE_SPEC=""
LIST_ONLY=0
WIDTH=1000
HEIGHT=600
LEFT=120
TOP=100

usage() {
  cat <<'EOF'
Usage: scripts/open_chrome_profile.sh [options] URL

Open URL in an existing personal Google Chrome profile without creating or
copying profile data.

Selection order:
  1. first currently-running original Chrome profile (no --user-data-dir)
  2. first existing profile from Chrome Local State profile.profiles_order
  3. Default, then Profile N in numeric order

Options:
  --profile NAME|PATH  Use a specific profile directory name or its full path.
  --list               Print discovered profiles and exit.
  --width N            Window width in logical points (default: 1000).
  --height N           Window height in logical points (default: 600).
  --left N              Window left position (default: 120).
  --top N               Window top position (default: 100).
  -h, --help           Show this help.

Examples:
  scripts/open_chrome_profile.sh https://ecooxai.github.io/drawpro/
  scripts/open_chrome_profile.sh --profile 'Profile 1' https://example.com/
  scripts/open_chrome_profile.sh --profile "$HOME/Library/Application Support/Google/Chrome/Profile 1" https://example.com/
EOF
}

while (($#)); do
  case "$1" in
    --profile) PROFILE_SPEC=${2:?missing value for --profile}; shift 2 ;;
    --list) LIST_ONLY=1; shift ;;
    --width) WIDTH=${2:?missing value for --width}; shift 2 ;;
    --height) HEIGHT=${2:?missing value for --height}; shift 2 ;;
    --left) LEFT=${2:?missing value for --left}; shift 2 ;;
    --top) TOP=${2:?missing value for --top}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --) shift; break ;;
    -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$URL" ]]; then echo "Only one URL is supported" >&2; exit 2; fi
      URL=$1; shift ;;
  esac
done

[[ -x "$CHROME_BIN" ]] || { echo "Chrome executable not found: $CHROME_BIN" >&2; exit 1; }
[[ -d "$CHROME_ROOT" ]] || { echo "Chrome profile root not found: $CHROME_ROOT" >&2; exit 1; }

# Emit existing profile directory names in Chrome's saved picker order, followed
# by valid on-disk fallbacks. This is read-only and never creates a profile.
profile_inventory() {
  python3 - "$CHROME_ROOT" <<'PY'
import json, pathlib, re, sys
root = pathlib.Path(sys.argv[1]).resolve()
state_path = root / 'Local State'
state = {}
try:
    state = json.loads(state_path.read_text())
except Exception:
    pass
order = state.get('profile', {}).get('profiles_order', [])
seen = set()
def valid(name):
    if not isinstance(name, str) or '/' in name or '\\' in name or name in ('', '.', '..'):
        return False
    p = root / name
    return p.is_dir() and (p / 'Preferences').is_file()
for name in order:
    if valid(name) and name not in seen:
        print(name); seen.add(name)
if valid('Default') and 'Default' not in seen:
    print('Default'); seen.add('Default')
numbered=[]
for p in root.iterdir():
    m=re.fullmatch(r'Profile (\d+)', p.name)
    if m and valid(p.name) and p.name not in seen:
        numbered.append((int(m.group(1)), p.name))
for _, name in sorted(numbered):
    print(name)
PY
}

# Prefer the user's already-running original Chrome profile. Processes using an
# explicit --user-data-dir belong to disposable/test roots and are excluded.
running_original_profile() {
  ps -axo pid=,command= | python3 -c '
import re, sys
for line in sys.stdin:
    if "/Google Chrome.app/Contents/MacOS/Google Chrome" not in line:
        continue
    if " --type=" in line or "--user-data-dir=" in line:
        continue
    m = re.search(r"(?:^| )--profile-directory=(.+?)(?= --[A-Za-z0-9-]+=| --[A-Za-z0-9-]+(?: |$)|$)", line)
    if m:
        value = m.group(1).strip().strip("\"\x27")
        print(value); break
'
}

PROFILES=()
while IFS= read -r profile; do
  PROFILES[${#PROFILES[@]}]="$profile"
done < <(profile_inventory)

if (( LIST_ONLY )); then
  running=$(running_original_profile || true)
  [[ -n "$running" ]] && printf 'running-original: %s\n' "$running"
  for p in "${PROFILES[@]}"; do
    printf '%s\t%s\n' "$p" "$CHROME_ROOT/$p"
  done
  exit 0
fi

[[ -n "$URL" ]] || { usage >&2; exit 2; }

if [[ -n "$PROFILE_SPEC" ]]; then
  if [[ "$PROFILE_SPEC" == /* ]]; then
    profile_path=$(python3 - "$PROFILE_SPEC" <<'PY'
import pathlib, sys
print(pathlib.Path(sys.argv[1]).expanduser().resolve())
PY
)
    root_real=$(python3 - "$CHROME_ROOT" <<'PY'
import pathlib, sys
print(pathlib.Path(sys.argv[1]).expanduser().resolve())
PY
)
    [[ "$(dirname "$profile_path")" == "$root_real" ]] || {
      echo "Profile path must be a direct child of Chrome root: $root_real" >&2; exit 2;
    }
    PROFILE=$(basename "$profile_path")
  else
    PROFILE=$PROFILE_SPEC
  fi
else
  PROFILE=$(running_original_profile || true)
  if [[ -z "$PROFILE" ]]; then
    ((${#PROFILES[@]})) || { echo "No existing Chrome profiles found" >&2; exit 1; }
    PROFILE=${PROFILES[0]}
  fi
fi

PROFILE_PATH="$CHROME_ROOT/$PROFILE"
[[ -d "$PROFILE_PATH" && -f "$PROFILE_PATH/Preferences" ]] || {
  echo "Existing Chrome profile not found: $PROFILE_PATH" >&2; exit 1;
}

printf 'profile=%s\nprofile_path=%s\nurl=%s\n' "$PROFILE" "$PROFILE_PATH" "$URL"

# Chrome expects --profile-directory relative to its normal user-data root.
# Passing PROFILE_PATH itself as --user-data-dir would create a nested Default
# profile, so we deliberately resolve/validate the full path but pass its
# basename here. Do not use -n: reuse the existing personal Chrome instance.
/usr/bin/open -g -a "Google Chrome" --args \
  --profile-directory="$PROFILE" \
  --new-window \
  --window-size="$WIDTH,$HEIGHT" \
  --window-position="$LEFT,$TOP" \
  "$URL"
