#!/bin/bash
# ============================================================================
# Xcode Tools for Zed — Setup Script
#
# Run once after installing the extension to configure Zed's tasks.json.
#
# What it does:
#   1. Copies helpers.sh to ~/.config/zed/xcode-tools/helpers.sh
#   2. Backs up existing ~/.config/zed/tasks.json
#   3. Writes Xcode Tools tasks to tasks.json
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
log_info "Step 1/3: Installing scripts"
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

# ── Step 2: Backup existing tasks.json ──
if [[ -f "$TASKS_FILE" ]]; then
    BACKUP="$TASKS_FILE.backup.$(date +%Y%m%d_%H%M%S)"
    log_info "Step 2/3: Backing up existing tasks.json"
    cp "$TASKS_FILE" "$BACKUP"
    log_success "Backup: $BACKUP"
else
    log_info "Step 2/3: No existing tasks.json — creating new"
fi

# ── Step 3: Write new tasks.json ──
log_info "Step 3/3: Configuring tasks.json"

HELPER_PATH_ESCAPED=$(echo "$HELPERS_DST" | sed 's/"/\\"/g')
INSTALL_DIR_ESCAPED=$(echo "$INSTALL_DIR" | sed 's/"/\\"/g')

cat > "$TASKS_FILE" << TASKEOF
[
  {
    "label": "\$ZED_CUSTOM_SWIFT_TEST_CLASS test",
    "command": "\"${HELPER_PATH_ESCAPED}\"",
    "args": [
      "inline-test",
      "--test-class",
      "\$ZED_CUSTOM_SWIFT_TEST_CLASS"
    ],
    "cwd": "\$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["swift-xctest-class", "swift-testing-suite"]
  },
  {
    "label": "\$ZED_CUSTOM_SWIFT_TEST_CLASS.\$ZED_CUSTOM_SWIFT_TEST_FUNC test",
    "command": "\"${HELPER_PATH_ESCAPED}\"",
    "args": [
      "inline-test",
      "--test-class",
      "\$ZED_CUSTOM_SWIFT_TEST_CLASS",
      "--test-func",
      "\$ZED_CUSTOM_SWIFT_TEST_FUNC"
    ],
    "cwd": "\$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["swift-xctest-func", "swift-testing-member-func"]
  },
  {
    "label": "\$ZED_CUSTOM_SWIFT_TEST_FUNC test",
    "command": "\"${HELPER_PATH_ESCAPED}\"",
    "args": [
      "inline-test",
      "--test-func",
      "\$ZED_CUSTOM_SWIFT_TEST_FUNC"
    ],
    "cwd": "\$ZED_WORKTREE_ROOT",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["swift-testing-bare-func"]
  },
  {
    "label": "Xcode: Build (Debug)",
    "command": "\"${HELPER_PATH_ESCAPED}\" build -c Debug",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Build (Release)",
    "command": "\"${HELPER_PATH_ESCAPED}\" build -c Release",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Build All (Debug)",
    "command": "\"${HELPER_PATH_ESCAPED}\" build -s all -c Debug",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Clean Build (Debug)",
    "command": "\"${HELPER_PATH_ESCAPED}\" build --clean -c Debug",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Clean Build (Release)",
    "command": "\"${HELPER_PATH_ESCAPED}\" build --clean -c Release",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-build"]
  },
  {
    "label": "Xcode: Run (macOS)",
    "command": "\"${HELPER_PATH_ESCAPED}\" run-macos",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-run"]
  },
  {
    "label": "Xcode: Run (Simulator)",
    "command": "\"${HELPER_PATH_ESCAPED}\" run-simulator",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-run"]
  },
  {
    "label": "Xcode: Test",
    "command": "\"${HELPER_PATH_ESCAPED}\" test",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-test"]
  },
  {
    "label": "Xcode: Clean",
    "command": "\"${HELPER_PATH_ESCAPED}\" clean",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-clean"]
  },
  {
    "label": "Xcode: Simulator — Stop App",
    "command": "\"${HELPER_PATH_ESCAPED}\" stop-simulator",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-simulator"]
  },
  {
    "label": "Xcode: Simulator — Shutdown All",
    "command": "\"${HELPER_PATH_ESCAPED}\" shutdown-simulator",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-simulator"]
  },
  {
    "label": "Xcode: List Schemes",
    "command": "\"${HELPER_PATH_ESCAPED}\" list",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-list"]
  },
  {
    "label": "Xcode: Run All Tests (helpers.sh)",
    "command": "bash \"${INSTALL_DIR_ESCAPED}/test_helpers.sh\" \"\$ZED_WORKTREE_ROOT\"",
    "use_new_terminal": false,
    "allow_concurrent_runs": false,
    "tags": ["xcode-test-suite"]
  }
]
TASKEOF

log_success "tasks.json configured"

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
echo "  - Xcode: Run All Tests (helpers.sh)"
echo ""
echo -e "In Zed: ${BOLD}Cmd+Shift+P${NC} → ${BOLD}task: spawn${NC} → ${BOLD}Xcode:${NC}"
echo ""
echo -e "${YELLOW}Note:${NC} When running on Simulator, you'll pick from available devices."
echo "To skip selection: export XCODE_TOOLS_SIMULATOR=\"iPhone 17 Pro\" (add to shell config)"
