#!/bin/bash
# ============================================================================
# Xcode Tools — Automated Test Script
#
# Usage:
#   bash scripts/test_helpers.sh /path/to/xcode-project
#
# Tests all helpers.sh actions (except xcodebuild test) against a real project.
# ============================================================================

set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

PASS=0
FAIL=0
SKIP=0

_test_pass() { echo -e "  ${GREEN}PASS${NC} $1"; ((PASS++)); }
_test_fail() { echo -e "  ${RED}FAIL${NC} $1: $2"; ((FAIL++)); }
_test_skip() { echo -e "  ${YELLOW}SKIP${NC} $1: $2"; ((SKIP++)); }

# ── Args ──
HELPERS="$(cd "$(dirname "$0")" && pwd)/helpers.sh"
PROJECT_DIR="${1:-}"

if [[ -z "$PROJECT_DIR" ]]; then
    echo "Usage: bash scripts/test_helpers.sh /path/to/xcode-project"
    echo ""
    echo "Example: bash scripts/test_helpers.sh ~/Project/MyApp"
    exit 1
fi

if [[ ! -d "$PROJECT_DIR" ]]; then
    echo -e "${RED}ERROR:${NC} Directory not found: $PROJECT_DIR"
    exit 1
fi

if [[ ! -f "$HELPERS" ]]; then
    echo -e "${RED}ERROR:${NC} helpers.sh not found at: $HELPERS"
    exit 1
fi

echo -e "${BOLD}${CYAN}"
echo "  ╔══════════════════════════════════════╗"
echo "  ║    Xcode Tools — Test Suite          ║"
echo "  ╚══════════════════════════════════════╝"
echo -e "${NC}"
echo -e "${BLUE}Project:${NC} $PROJECT_DIR"
echo -e "${BLUE}Helpers:${NC} $HELPERS"
echo ""

cd "$PROJECT_DIR"

# ============================================================================
# Test 1: List Schemes
# ============================================================================
echo -e "${BOLD}[1/7] List Schemes${NC}"
OUTPUT=$("$HELPERS" list 2>&1)
EXIT_CODE=$?
# Strip ANSI color codes for parsing
CLEAN_OUTPUT=$(echo "$OUTPUT" | sed 's/\x1b\[[0-9;]*m//g')
if [[ $EXIT_CODE -eq 0 ]]; then
    SCHEME=$(echo "$CLEAN_OUTPUT" | grep -E '^\s+[0-9]+\)' | head -1 | sed 's/.*) //' | xargs)
    if [[ -z "$SCHEME" ]]; then
        SCHEME=$(echo "$CLEAN_OUTPUT" | grep -i "scheme:" | head -1 | sed 's/.*Scheme: //' | xargs)
    fi
    if [[ -n "$SCHEME" ]]; then
        _test_pass "Found scheme: $SCHEME"
    else
        _test_fail "List Schemes" "No schemes found in output"
        echo "$CLEAN_OUTPUT"
    fi
else
    _test_fail "List Schemes" "Exit code: $EXIT_CODE"
    echo "$CLEAN_OUTPUT"
fi
echo ""

if [[ -z "$SCHEME" ]]; then
    echo -e "${RED}Cannot continue without a scheme. Aborting.${NC}"
    exit 1
fi

echo -e "${BLUE}Using scheme:${NC} $SCHEME"
echo ""

# ============================================================================
# Test 2: Build (Debug)
# ============================================================================
echo -e "${BOLD}[2/7] Build (Debug)${NC}"
OUTPUT=$("$HELPERS" build -s "$SCHEME" -c Debug 2>&1)
EXIT_CODE=$?
if [[ $EXIT_CODE -eq 0 ]]; then
    if echo "$OUTPUT" | grep -q "SUCCEEDED"; then
        _test_pass "Build succeeded"
        # Check if products dir was mentioned
        PRODUCTS_LINE=$(echo "$OUTPUT" | grep "Products:" | sed 's/.*Products: //' | sed 's/\x1b\[[0-9;]*m//g')
        if [[ -n "$PRODUCTS_LINE" ]]; then
            _test_pass "Products dir: $PRODUCTS_LINE"
        fi
    else
        _test_fail "Build" "No SUCCEEDED in output"
    fi
else
    CLEAN_ERR=$(echo "$OUTPUT" | sed 's/\x1b\[[0-9;]*m//g')
    # Extract actual error lines (xcodebuild error: ... format)
    ERRORS=$(echo "$CLEAN_ERR" | grep -i "error:" | grep -v "^Command " | head -5)
    if [[ -n "$ERRORS" ]]; then
        _test_fail "Build" "Exit code: $EXIT_CODE"
        echo -e "  ${RED}Reason:${NC}"
        echo "$ERRORS" | while IFS= read -r line; do echo "    $line"; done
    else
        _test_fail "Build" "Exit code: $EXIT_CODE"
        echo "$CLEAN_ERR" | tail -5
    fi
fi
echo ""

# ============================================================================
# Test 3: Clean
# ============================================================================
echo -e "${BOLD}[3/7] Clean${NC}"
OUTPUT=$("$HELPERS" clean -s "$SCHEME" 2>&1)
EXIT_CODE=$?
if [[ $EXIT_CODE -eq 0 ]]; then
    if echo "$OUTPUT" | grep -q "Clean completed\|CLEAN SUCCEEDED\|succeeded"; then
        _test_pass "Clean succeeded"
    else
        _test_pass "Clean finished (exit 0)"
    fi
else
    _test_fail "Clean" "Exit code: $EXIT_CODE"
    echo "$OUTPUT" | tail -3
fi
echo ""

# ============================================================================
# Test 4: Build again (for run test)
# ============================================================================
echo -e "${BOLD}[4/7] Rebuild for Run test${NC}"
OUTPUT=$("$HELPERS" build -s "$SCHEME" -c Debug 2>&1)
EXIT_CODE=$?
if [[ $EXIT_CODE -eq 0 ]]; then
    _test_pass "Rebuild succeeded"
else
    CLEAN_ERR=$(echo "$OUTPUT" | sed 's/\x1b\[[0-9;]*m//g')
    ERRORS=$(echo "$CLEAN_ERR" | grep -i "error:" | grep -v "^Command " | head -5)
    if [[ -n "$ERRORS" ]]; then
        _test_fail "Rebuild" "Exit code: $EXIT_CODE"
        echo -e "  ${RED}Reason:${NC}"
        echo "$ERRORS" | while IFS= read -r line; do echo "    $line"; done
    else
        _test_fail "Rebuild" "Exit code: $EXIT_CODE"
        echo "$CLEAN_ERR" | tail -5
    fi
    echo ""
    echo -e "${YELLOW}Skipping run tests since build failed.${NC}"
    SKIP=$((SKIP + 2))
    # Jump to summary
    echo ""
    echo -e "${BOLD}[5/7] Run (macOS)${NC}"
    _test_skip "Run macOS" "Build failed"
    echo ""
    echo -e "${BOLD}[6/7] Run (Simulator)${NC}"
    _test_skip "Run Simulator" "Build failed"
    echo ""
    # Go to shutdown test
    goto_shutdown=true
fi
echo ""

# ============================================================================
# Test 5: Run macOS (build + open)
# ============================================================================
if [[ "${goto_shutdown:-false}" != "true" ]]; then
    echo -e "${BOLD}[5/7] Run macOS${NC}"
    # Run in subshell, capture output, kill the opened app after 3 seconds
    OUTPUT=$("$HELPERS" run-macos -s "$SCHEME" -c Debug 2>&1)
    EXIT_CODE=$?
    if [[ $EXIT_CODE -eq 0 ]]; then
        if echo "$OUTPUT" | grep -q "Running:\|SUCCEEDED"; then
            _test_pass "Run macOS succeeded (app opened)"
            # Try to close the app
            APP_NAME=$(echo "$OUTPUT" | grep "Running:" | sed 's/.*Running: //' | sed 's/\x1b\[[0-9;]*m//g' | xargs basename 2>/dev/null | sed 's/\.app//')
            if [[ -n "$APP_NAME" ]]; then
                sleep 2
                osascript -e "tell application \"$APP_NAME\" to quit" 2>/dev/null || true
                _test_pass "App closed: $APP_NAME"
            fi
        else
            _test_pass "Run macOS finished (exit 0)"
        fi
    else
        CLEAN_ERR=$(echo "$OUTPUT" | sed 's/\x1b\[[0-9;]*m//g')
        if echo "$CLEAN_ERR" | grep -q "App not found"; then
            _test_skip "Run macOS" "No .app target (library/framework project?)"
        else
            ERRORS=$(echo "$CLEAN_ERR" | grep -i "error:" | grep -v "^Command " | head -3)
            _test_fail "Run macOS" "Exit code: $EXIT_CODE"
            if [[ -n "$ERRORS" ]]; then
                echo -e "  ${RED}Reason:${NC}"
                echo "$ERRORS" | while IFS= read -r line; do echo "    $line"; done
            fi
        fi
    fi
    echo ""

    # ============================================================================
    # Test 6: Run Simulator
    # ============================================================================
    echo -e "${BOLD}[6/7] Run Simulator${NC}"
    # Find first available iPhone simulator
    SIM_NAME=$(xcrun simctl list devices available -j 2>/dev/null \
        | sed -n 's/.*"name" *: *"\([^"]*\)".*/\1/p' \
        | grep -i "iphone" | head -1)

    if [[ -n "$SIM_NAME" ]]; then
        echo -e "  ${BLUE}Simulator:${NC} $SIM_NAME"
        OUTPUT=$(XCODE_TOOLS_SIMULATOR="$SIM_NAME" "$HELPERS" run-simulator -s "$SCHEME" -c Debug 2>&1)
        EXIT_CODE=$?
        if [[ $EXIT_CODE -eq 0 ]]; then
            if echo "$OUTPUT" | grep -q "Launched:"; then
                _test_pass "Simulator launch succeeded"
                # Stop the app
                sleep 2
                "$HELPERS" stop-simulator -s "$SCHEME" 2>&1 >/dev/null
            else
                _test_pass "Simulator finished (exit 0)"
            fi
        else
            CLEAN_ERR=$(echo "$OUTPUT" | sed 's/\x1b\[[0-9;]*m//g')
            if echo "$CLEAN_ERR" | grep -q "Supported platforms.*empty\|Unable to find.*device\|Available destinations"; then
                _test_skip "Run Simulator" "Project does not support iOS Simulator"
            else
                ERRORS=$(echo "$CLEAN_ERR" | grep -i "error:" | grep -v "^Command " | head -3)
                _test_fail "Run Simulator" "Exit code: $EXIT_CODE"
                if [[ -n "$ERRORS" ]]; then
                    echo -e "  ${RED}Reason:${NC}"
                    echo "$ERRORS" | while IFS= read -r line; do echo "    $line"; done
                else
                    echo "$CLEAN_ERR" | tail -5
                fi
            fi
        fi
    else
        _test_skip "Run Simulator" "No iPhone simulator available"
    fi
    echo ""
fi

# ============================================================================
# Test 7: Shutdown Simulators
# ============================================================================
echo -e "${BOLD}[7/7] Shutdown Simulators${NC}"
OUTPUT=$("$HELPERS" shutdown-simulator 2>&1)
EXIT_CODE=$?
if [[ $EXIT_CODE -eq 0 ]]; then
    _test_pass "Shutdown simulators succeeded"
else
    _test_fail "Shutdown" "Exit code: $EXIT_CODE"
fi
echo ""

# ============================================================================
# Summary
# ============================================================================
echo -e "${BOLD}${CYAN}═══════════════════════════════════════${NC}"
TOTAL=$((PASS + FAIL + SKIP))
echo -e "${BOLD}Results:${NC} ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}, ${YELLOW}${SKIP} skipped${NC} / ${TOTAL} total"

if [[ $FAIL -eq 0 ]]; then
    echo -e "${BOLD}${GREEN}All tests passed!${NC}"
    exit 0
else
    echo -e "${BOLD}${RED}Some tests failed.${NC}"
    exit 1
fi
