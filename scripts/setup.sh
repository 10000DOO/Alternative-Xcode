#!/bin/bash
# ============================================================================
# Xcode Tools for Zed — Setup Script
#
# Run once after installing the extension to configure Zed's tasks.json.
#
# What it does:
#   1. Copies helpers.sh / shim / bsp binary to ~/.config/zed/xcode-tools/
#   2. Builds xcode-bsp when cargo is available
#   3. Backs up existing ~/.config/zed/tasks.json
#   4. Writes portable Xcode Tools tasks ($HOME-based, not /Users/<name>/)
#   5. Points settings.json sourcekit-lsp at the installed shim (absolute path)
#
# Usage:
#   bash scripts/setup.sh
# ============================================================================

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }

echo -e "${BOLD}${CYAN}"
echo "  ╔══════════════════════════════════════╗"
echo "  ║    Xcode Tools for Zed — Setup       ║"
echo "  ╚══════════════════════════════════════╝"
echo -e "${NC}"

# ── Paths ──
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HELPERS_SRC="$SCRIPT_DIR/helpers.sh"
ZED_CONFIG_DIR="$HOME/.config/zed"
INSTALL_DIR="$ZED_CONFIG_DIR/xcode-tools"
HELPERS_DST="$INSTALL_DIR/helpers.sh"
TASKS_FILE="$ZED_CONFIG_DIR/tasks.json"

# ── Check helpers.sh exists ──
if [[ ! -f "$HELPERS_SRC" ]]; then
    log_error "helpers.sh not found at: $HELPERS_SRC"
    log_error "Run this script from the extension root: bash scripts/setup.sh"
    exit 1
fi

# ── Step 1: Copy helpers.sh ──
log_info "Step 1/5: Installing scripts"
mkdir -p "$INSTALL_DIR"
cp "$HELPERS_SRC" "$HELPERS_DST"
chmod +x "$HELPERS_DST"
log_success "Installed: $HELPERS_DST"
if [[ -f "$SCRIPT_DIR/test_helpers.sh" ]]; then
    cp "$SCRIPT_DIR/test_helpers.sh" "$INSTALL_DIR/test_helpers.sh"
    chmod +x "$INSTALL_DIR/test_helpers.sh"
    log_success "Installed: $INSTALL_DIR/test_helpers.sh"
fi

# ── Check: xcbeautify (optional, not auto-installed) ──
if command -v xcbeautify &>/dev/null; then
    log_success "xcbeautify detected (prettier live output enabled)"
else
    log_info "xcbeautify not found — live build output will use a basic filter."
    log_info "       (Optional) For prettier output:  brew install xcbeautify"
    log_info "       Error summary works fine without it."
fi

# ── Step 2: Build & install LSP provider binary (non-fatal) ──
log_info "Step 2/5: Building LSP provider (xcode-bsp)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
if ! command -v cargo &>/dev/null; then
    log_warn "cargo not found — skipping LSP provider build."
    log_warn "       Build/run/test/debug features work without it."
    log_warn "       To enable the LSP provider later, install Rust (https://rustup.rs) and re-run setup."
elif cargo build --release -p xcode-bsp-server --manifest-path "$REPO_ROOT/Cargo.toml"; then
    mkdir -p "$INSTALL_DIR/bin"
    cp "$REPO_ROOT/target/release/xcode-bsp" "$INSTALL_DIR/bin/xcode-bsp"
    chmod +x "$INSTALL_DIR/bin/xcode-bsp"
    log_success "Installed: $INSTALL_DIR/bin/xcode-bsp"
else
    log_warn "LSP provider build failed — continuing without it."
fi

# ── Install sourcekit-lsp shim ──
# Wraps sourcekit-lsp to strip the empty `params: {}` from its params-less
# `workspace/*/refresh` requests, which Zed otherwise rejects
# ("invalid type: map, expected unit") — that rejection stops Zed from
# re-requesting semantic tokens, killing Xcode-accurate highlighting.
# Point lsp.sourcekit-lsp.binary at: /usr/bin/python3 with this script as arg.
mkdir -p "$INSTALL_DIR/bin"
if cp "$SCRIPT_DIR/sourcekit-lsp-shim.py" "$INSTALL_DIR/bin/sourcekit-lsp-shim"; then
    chmod +x "$INSTALL_DIR/bin/sourcekit-lsp-shim"
    log_success "Installed: $INSTALL_DIR/bin/sourcekit-lsp-shim"
else
    log_warn "Could not install sourcekit-lsp shim — semantic highlighting may not update."
fi

# ── Step 3: Backup existing tasks.json ──
if [[ -f "$TASKS_FILE" ]]; then
    BACKUP="$TASKS_FILE.backup.$(date +%Y%m%d_%H%M%S)"
    log_info "Step 3/5: Backing up existing tasks.json"
    cp "$TASKS_FILE" "$BACKUP"
    log_success "Backup: $BACKUP"
else
    log_info "Step 3/5: No existing tasks.json — creating new"
fi

# ── Step 4: Write portable tasks.json ──
# Use $HOME (expanded by Zed at task run time) so the same tasks.json works on
# any machine/account without re-baking absolute /Users/<name>/... paths.
log_info "Step 4/5: Configuring tasks.json (portable \$HOME paths)"

cat > "$TASKS_FILE" << 'TASKEOF'
[
  {
    "label": "$ZED_CUSTOM_SWIFT_TEST_CLASS test",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": [
      "inline-test",
      "--test-class",
      "$ZED_CUSTOM_SWIFT_TEST_CLASS"
    ],
    "cwd": "$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["swift-xctest-class", "swift-testing-suite"]
  },
  {
    "label": "$ZED_CUSTOM_SWIFT_TEST_CLASS.$ZED_CUSTOM_SWIFT_TEST_FUNC test",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": [
      "inline-test",
      "--test-class",
      "$ZED_CUSTOM_SWIFT_TEST_CLASS",
      "--test-func",
      "$ZED_CUSTOM_SWIFT_TEST_FUNC"
    ],
    "cwd": "$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["swift-xctest-func", "swift-testing-member-func"]
  },
  {
    "label": "$ZED_CUSTOM_SWIFT_TEST_FUNC test",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": [
      "inline-test",
      "--test-func",
      "$ZED_CUSTOM_SWIFT_TEST_FUNC"
    ],
    "cwd": "$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["swift-testing-bare-func"]
  },
  {
    "label": "Xcode: Build (Debug)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["build", "-c", "Debug"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Build (Release)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["build", "-c", "Release"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Build All (Debug)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["build", "-s", "all", "-c", "Debug"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Clean Build (Debug)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["build", "--clean", "-c", "Debug"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Clean Build (Release)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["build", "--clean", "-c", "Release"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Run (macOS)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["run-macos"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-run"]
  },
  {
    "label": "Xcode: Run (Simulator)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["run-simulator"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-run"]
  },
  {
    "label": "Xcode: Test",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["test"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-test"]
  },
  {
    "label": "Xcode: Clean",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["clean"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-clean"]
  },
  {
    "label": "Xcode: Simulator — Stop App",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["stop-simulator"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-simulator"]
  },
  {
    "label": "Xcode: Simulator — Shutdown All",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["shutdown-simulator"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-simulator"]
  },
  {
    "label": "Xcode: List Schemes",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["list"],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-list"]
  },
  {
    "label": "Xcode: Setup LSP (BSP)",
    "command": "$HOME/.config/zed/xcode-tools/helpers.sh",
    "args": ["bsp-setup"],
    "cwd": "$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-lsp"]
  },
  {
    "label": "Xcode: Run All Tests (helpers.sh)",
    "command": "bash",
    "args": [
      "$HOME/.config/zed/xcode-tools/test_helpers.sh",
      "$ZED_WORKTREE_ROOT"
    ],
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-test-suite"]
  }
]
TASKEOF

log_success "tasks.json configured (portable \$HOME paths)"

# ── Step 5: Wire sourcekit-lsp shim in settings.json ──
# Zed settings do not expand $HOME in lsp.binary.path, so we write the absolute
# path for *this* user at setup time. Re-run setup on each machine after sync.
SETTINGS_FILE="$ZED_CONFIG_DIR/settings.json"
SHIM_DST="$INSTALL_DIR/bin/sourcekit-lsp-shim"
log_info "Step 5/5: Configuring settings.json (sourcekit-lsp shim)"

if [[ -x "$SHIM_DST" ]]; then
    if [[ -f "$SETTINGS_FILE" ]]; then
        cp "$SETTINGS_FILE" "$SETTINGS_FILE.backup.$(date +%Y%m%d_%H%M%S)"
    fi
    # shellcheck disable=SC2016
    python3 - "$SETTINGS_FILE" "$SHIM_DST" <<'PY'
import json, re, sys
from pathlib import Path

settings_path = Path(sys.argv[1])
shim_path = sys.argv[2]

binary = {
    "path": "/usr/bin/python3",
    "arguments": [shim_path],
}

def strip_jsonc(text: str) -> str:
    # Drop // line comments and /* */ blocks outside strings (good enough for Zed settings).
    out = []
    i, n = 0, len(text)
    in_str = False
    esc = False
    while i < n:
        c = text[i]
        if in_str:
            out.append(c)
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True
            out.append(c)
            i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            i += 2
            while i + 1 < n and not (text[i] == "*" and text[i + 1] == "/"):
                i += 1
            i = min(i + 2, n)
            continue
        out.append(c)
        i += 1
    return "".join(out)

if settings_path.exists():
    raw = settings_path.read_text(encoding="utf-8")
    try:
        data = json.loads(strip_jsonc(raw))
    except json.JSONDecodeError as e:
        print(f"WARN: could not parse settings.json ({e}); leaving file unchanged", file=sys.stderr)
        sys.exit(0)
else:
    data = {}
    raw = ""

lsp = data.setdefault("lsp", {})
sk = lsp.setdefault("sourcekit-lsp", {})
sk["binary"] = binary

# Prefer a clean rewrite when the original had no comments, else rewrite whole JSON.
# Comments in the original file will be lost; a .backup.* was made above.
settings_path.parent.mkdir(parents=True, exist_ok=True)
settings_path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
print(f"OK: sourcekit-lsp binary -> python3 {shim_path}")
PY
    if [[ $? -eq 0 ]]; then
        log_success "settings.json: sourcekit-lsp → $SHIM_DST"
        log_info "       (settings uses an absolute path; re-run setup after copying config to another Mac)"
    else
        log_warn "Could not update settings.json — set lsp.sourcekit-lsp.binary manually"
    fi
else
    log_warn "Shim not installed — skipped settings.json update"
fi

# ── Done ──
echo ""
echo -e "${BOLD}${GREEN}Setup complete!${NC}"
echo ""
echo "Registered Tasks:"
echo "  - Xcode: Build (Debug / Release)"
echo "  - Xcode: Build All (Debug)"
echo "  - Xcode: Clean Build (Debug / Release)"
echo "  - Xcode: Run (macOS / Simulator)"
echo "  - Xcode: Test"
echo "  - Xcode: Clean"
echo "  - Xcode: Simulator — Stop App"
echo "  - Xcode: Simulator — Shutdown All"
echo "  - Xcode: List Schemes"
echo "  - Xcode: Setup LSP (BSP)"
echo "  - Xcode: Run All Tests (helpers.sh)"
echo ""
echo "Paths are portable:"
echo "  - tasks.json uses \$HOME/.config/zed/xcode-tools/... (sync-safe)"
echo "  - settings.json lsp.binary is set to this machine's absolute shim path"
echo "  - On a new Mac: clone + bash scripts/setup.sh (do not rely on copied absolute paths)"
echo ""
echo "LSP: xcode-bsp provider binary built & installed to $INSTALL_DIR/bin (skipped if Rust/cargo not present)"
echo "LSP: sourcekit-lsp shim at $INSTALL_DIR/bin/sourcekit-lsp-shim (semantic token refresh fix)"
echo ""
echo -e "In Zed: ${BOLD}Cmd+Shift+P${NC} → ${BOLD}task: spawn${NC} → ${BOLD}Xcode:${NC}"
echo ""
echo -e "${YELLOW}Note:${NC} When running on Simulator, you'll pick from available devices."
echo "To skip selection: export XCODE_TOOLS_SIMULATOR=\"iPhone 17 Pro\" (add to shell config)"
