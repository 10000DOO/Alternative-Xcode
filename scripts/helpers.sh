#!/bin/bash
# ============================================================================
# Xcode Tools for Zed — helpers.sh
# xbuild compatible + run/test/clean extensions
#
# Usage (standalone):
#   helpers.sh build                    # Interactive, Debug
#   helpers.sh build -s MyScheme        # Build specific scheme
#   helpers.sh build -c Release         # Release build
#   helpers.sh build -s all             # Build all schemes
#   helpers.sh build --clean            # Clean build
#   helpers.sh run-macos                # Build & Run (macOS)
#   helpers.sh run-simulator            # Build & Run (Simulator)
#   helpers.sh test                     # Run tests
#   helpers.sh clean                    # Clean build products
#   helpers.sh list                     # List schemes
#
# Usage (from Zed tasks.json):
#   "command": "path/to/helpers.sh build -c Debug"
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(pwd)"

# ── Settings (env vars with defaults) ──
XCODE_TOOLS_CONFIG="${XCODE_TOOLS_CONFIG:-Debug}"
XCODE_TOOLS_SIMULATOR="${XCODE_TOOLS_SIMULATOR:-}"

# ── Internal state ──
_BUILD_TARGET=""
_BUILD_TARGET_FLAG=""
_PRODUCTS_DIR=""
_PRODUCT_NAME=""
_BUNDLE_ID=""

# ── Colors ──
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

_log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
_log_success() { echo -e "${GREEN}[OK]${NC} $1"; }
_log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
_log_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
_log_step()    { echo -e "\n${BOLD}${CYAN}=== $1 ===${NC}"; }

# ============================================================================
# Set PATH to Xcode's default
# ============================================================================
DEVELOPER_BIN="$(xcode-select -p 2>/dev/null)/usr/bin"
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:${DEVELOPER_BIN}:${PATH}"

# ============================================================================
# Project Detection
# ============================================================================
_detect_project() {
    local workspaces=()
    while IFS= read -r -d '' ws; do workspaces+=("$ws")
    done < <(find "$SCRIPT_DIR" -maxdepth 1 -name "*.xcworkspace" ! -path "*.xcodeproj/*" -print0 2>/dev/null)

    if [[ ${#workspaces[@]} -eq 1 ]]; then
        _BUILD_TARGET="${workspaces[0]}"; _BUILD_TARGET_FLAG="-workspace"; return
    elif [[ ${#workspaces[@]} -gt 1 ]]; then
        _log_warn "Multiple workspaces found:"; for ws in "${workspaces[@]}"; do echo "  $(basename "$ws")"; done
        _log_error "Use -w to specify one"; exit 1
    fi

    local projects=()
    while IFS= read -r -d '' pj; do projects+=("$pj")
    done < <(find "$SCRIPT_DIR" -maxdepth 2 -name "*.xcodeproj" -print0 2>/dev/null)

    if [[ ${#projects[@]} -eq 1 ]]; then
        _BUILD_TARGET="${projects[0]}"; _BUILD_TARGET_FLAG="-project"; return
    elif [[ ${#projects[@]} -gt 1 ]]; then
        _log_warn "Multiple projects found:"; for pj in "${projects[@]}"; do echo "  $(basename "$pj")"; done
        _log_error "Use -p to specify one"; exit 1
    fi

    _log_error "No .xcworkspace or .xcodeproj found in $(pwd)"; exit 1
}

# ============================================================================
# Scheme Discovery (shared + user)
# ============================================================================
_discover_schemes() {
    # Primary: find .xcscheme files (shared + user)
    {
        find "$SCRIPT_DIR" -path "*/xcshareddata/xcschemes/*.xcscheme" 2>/dev/null
        find "$SCRIPT_DIR" -path "*/xcuserdata/*/xcschemes/*.xcscheme" 2>/dev/null
    } | while IFS= read -r f; do basename "$f" .xcscheme; done | sort -u
}

_discover_schemes_fallback() {
    # Fallback: xcodebuild -list (for projects without .xcscheme files)
    xcodebuild "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" -list 2>/dev/null \
        | sed -n '/Schemes:/,/^$/p' | grep -v 'Schemes:' | sed 's/^[[:space:]]*//' | grep -v '^$'
}

_select_scheme() {
    local schemes=()
    while IFS= read -r s; do
        [[ -n "$s" ]] && schemes+=("$s")
    done < <(_discover_schemes)

    # Fallback if no .xcscheme files found
    if [[ ${#schemes[@]} -eq 0 ]]; then
        _log_warn "No .xcscheme files found, trying xcodebuild -list..." >&2
        while IFS= read -r s; do
            [[ -n "$s" ]] && schemes+=("$s")
        done < <(_discover_schemes_fallback)
    fi

    if [[ ${#schemes[@]} -eq 0 ]]; then
        _log_error "No schemes found" >&2
        _log_info "Open the project in Xcode once to generate schemes." >&2
        exit 1
    fi

    if [[ ${#schemes[@]} -eq 1 ]]; then
        _log_info "Scheme: ${schemes[0]}" >&2
        echo "${schemes[0]}"; return
    fi

    _log_step "Available Schemes ($(basename "$_BUILD_TARGET"))" >&2
    for i in "${!schemes[@]}"; do
        echo -e "  ${BOLD}$((i+1))${NC}) ${schemes[$i]}" >&2
    done
    echo "" >&2
    read -rp "Select scheme number (or 'all'): " selection
    if [[ "$selection" == "all" ]]; then
        echo "all"; return
    elif [[ "$selection" =~ ^[0-9]+$ ]] && (( selection >= 1 && selection <= ${#schemes[@]} )); then
        echo "${schemes[$((selection-1))]}"; return
    else
        _log_error "Invalid selection" >&2; exit 1
    fi
}

# ============================================================================
# Simulator Selection
# ============================================================================
_select_simulator() {
    local simulators=()
    while IFS= read -r line; do
        [[ -n "$line" ]] && simulators+=("$line")
    done < <(xcrun simctl list devices available -j 2>/dev/null \
        | sed -n 's/.*"name" *: *"\([^"]*\)".*/\1/p' \
        | grep -i "iphone\|ipad" | sort -u)

    if [[ ${#simulators[@]} -eq 0 ]]; then
        _log_error "No available simulators found" >&2
        _log_info "Install iOS Simulator via Xcode → Settings → Platforms." >&2
        exit 1
    fi

    _log_step "Available Simulators" >&2
    for i in "${!simulators[@]}"; do
        echo -e "  ${BOLD}$((i+1))${NC}) ${simulators[$i]}" >&2
    done
    echo "" >&2
    read -rp "Select simulator number: " selection
    if [[ "$selection" =~ ^[0-9]+$ ]] && (( selection >= 1 && selection <= ${#simulators[@]} )); then
        echo "${simulators[$((selection-1))]}"; return
    else
        _log_error "Invalid selection" >&2; exit 1
    fi
}

# ============================================================================
# Build Settings Cache (single call)
# ============================================================================
_cache_build_settings() {
    local target_flag="$1" target="$2" scheme="$3" dest="${4:-}"
    local settings
    settings=$(xcodebuild "$target_flag" "$target" \
        -scheme "$scheme" -configuration "$XCODE_TOOLS_CONFIG" \
        ${dest:+-destination "$dest"} \
        -showBuildSettings 2>/dev/null)

    _PRODUCTS_DIR=$(echo "$settings" | grep '^\s*BUILT_PRODUCTS_DIR\s*=' | head -1 | sed 's/.*= *//')
    _PRODUCT_NAME=$(echo "$settings" | grep '^\s*PRODUCT_NAME\s*=' | head -1 | sed 's/.*= *//')
    _BUNDLE_ID=$(echo "$settings" | grep '^\s*PRODUCT_BUNDLE_IDENTIFIER\s*=' | head -1 | sed 's/.*= *//')
}

# ============================================================================
# Run command with optional xcbeautify + log capture for error reporting
# ============================================================================
_LAST_LOG="/tmp/xcode-tools-last-build.log"

_run_cmd() {
    if command -v xcbeautify &>/dev/null; then
        "$@" 2>&1 | tee "$_LAST_LOG" | xcbeautify
        return ${PIPESTATUS[0]}
    else
        "$@" 2>&1 | tee "$_LAST_LOG"
        return ${PIPESTATUS[0]}
    fi
}

_show_errors() {
    local context="$1"
    if [[ -f "$_LAST_LOG" ]]; then
        local errors
        errors=$(grep -E "(: error:|: fatal error:)" "$_LAST_LOG" | grep -v "^Command " | head -10)
        if [[ -n "$errors" ]]; then
            echo ""
            _log_error "=== Errors ($context) ==="
            echo "$errors"
        fi
        local warn_count
        warn_count=$(grep -cE ": warning:" "$_LAST_LOG" 2>/dev/null || echo "0")
        [[ "$warn_count" -gt 0 ]] && _log_warn "Warnings: ${warn_count}"
    fi
}

# ============================================================================
# Actions
# ============================================================================

# --- Build ---
action_build() {
    local config="$XCODE_TOOLS_CONFIG"
    local scheme=""
    local clean=false
    local all_schemes=false
    local explicit_workspace=""
    local explicit_project=""

    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme)  scheme="$2"; shift 2 ;;
            -c|--config)  config="$2"; shift 2 ;;
            -w|--workspace) explicit_workspace="$2"; shift 2 ;;
            -p|--project) explicit_project="$2"; shift 2 ;;
            --clean)      clean=true; shift ;;
            -l|--list)    action_list; exit 0 ;;
            *) shift ;;
        esac
    done

    XCODE_TOOLS_CONFIG="$config"

    if [[ -n "$explicit_workspace" ]]; then
        _BUILD_TARGET="$explicit_workspace"; _BUILD_TARGET_FLAG="-workspace"
    elif [[ -n "$explicit_project" ]]; then
        _BUILD_TARGET="$explicit_project"; _BUILD_TARGET_FLAG="-project"
    else
        _detect_project
    fi

    _log_info "Project: $(basename "$_BUILD_TARGET")"

    if [[ -z "$scheme" ]]; then
        scheme=$(_select_scheme)
    fi

    if [[ "$scheme" == "all" ]]; then
        local schemes=()
        while IFS= read -r s; do [[ -n "$s" ]] && schemes+=("$s"); done < <(_discover_schemes)
        local failed=() succeeded=()
        for s in "${schemes[@]}"; do
            if _build_one "$s" "$config" "$clean"; then succeeded+=("$s"); else failed+=("$s"); fi
        done
        echo ""
        [[ ${#succeeded[@]} -gt 0 ]] && _log_success "Succeeded: ${succeeded[*]}"
        [[ ${#failed[@]} -gt 0 ]] && _log_error "Failed: ${failed[*]}" && exit 1
    else
        _build_one "$scheme" "$config" "$clean"
    fi
}

_build_one() {
    local scheme="$1" config="$2" clean="$3"
    _log_step "Building: ${scheme} (${config})"

    local cmd=(xcodebuild "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET"
        -scheme "$scheme" -configuration "$config"
        -allowProvisioningUpdates)
    [[ "$clean" == "true" ]] && cmd+=(clean)
    cmd+=(build)

    _log_info "${cmd[*]}"
    echo ""
    _run_cmd "${cmd[@]}"
    local exit_code=$?

    if [[ $exit_code -eq 0 ]]; then
        _log_success "SUCCEEDED: ${scheme} (${config})"
        _cache_build_settings "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" "$scheme"
        if [[ -n "$_PRODUCTS_DIR" ]] && [[ -d "$_PRODUCTS_DIR" ]]; then
            _log_info "Products: $_PRODUCTS_DIR"
            open "$_PRODUCTS_DIR"
        fi
    else
        _log_error "FAILED: ${scheme} (${config})"
        _show_errors "Build"
    fi
    return $exit_code
}

# --- Run macOS ---
action_run_macos() {
    local config="$XCODE_TOOLS_CONFIG"
    local scheme=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme) scheme="$2"; shift 2 ;;
            -c|--config) config="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    XCODE_TOOLS_CONFIG="$config"

    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"
    [[ -z "$scheme" ]] && scheme=$(_select_scheme)

    _log_step "Building: ${scheme} (macOS, ${config})"
    _run_cmd xcodebuild build "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" \
        -scheme "$scheme" -configuration "$config" \
        -destination 'platform=macOS' \
        -allowProvisioningUpdates
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        _log_error "FAILED: Build ${scheme} (macOS, ${config})"
        _show_errors "Run macOS — Build"
        exit $exit_code
    fi

    _cache_build_settings "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" "$scheme"
    local app="$_PRODUCTS_DIR/$_PRODUCT_NAME.app"

    if [[ -d "$app" ]]; then
        _log_step "Running: $app"
        open "$app"
    else
        _log_error "App not found: $app"
        _log_info "This scheme may not produce a .app (library/framework target)."
        [[ -n "$_PRODUCTS_DIR" ]] && _log_info "Products dir: $_PRODUCTS_DIR" && open "$_PRODUCTS_DIR"
        exit 1
    fi
}

# --- Run Simulator ---
action_run_simulator() {
    local config="$XCODE_TOOLS_CONFIG"
    local scheme=""
    local simulator="$XCODE_TOOLS_SIMULATOR"
    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme)    scheme="$2"; shift 2 ;;
            -c|--config)    config="$2"; shift 2 ;;
            -d|--device)    simulator="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    XCODE_TOOLS_CONFIG="$config"

    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"
    [[ -z "$scheme" ]] && scheme=$(_select_scheme)

    # If no simulator specified, show picker
    if [[ -z "$simulator" ]]; then
        simulator=$(_select_simulator)
    fi

    local dest="platform=iOS Simulator,name=$simulator"
    _log_step "Building: ${scheme} → ${simulator} (${config})"
    _run_cmd xcodebuild build "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" \
        -scheme "$scheme" -configuration "$config" \
        -destination "$dest" \
        -allowProvisioningUpdates
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        _log_error "FAILED: Build ${scheme} → ${simulator} (${config})"
        _show_errors "Run Simulator — Build"
        exit $exit_code
    fi

    _cache_build_settings "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" "$scheme" "$dest"

    _log_step "Launching on ${simulator}"
    if ! xcrun simctl boot "$simulator" 2>/dev/null; then
        # Already booted is fine, but check if device exists
        if ! xcrun simctl list devices | grep -q "$simulator"; then
            _log_error "Simulator not found: $simulator"
            _log_info "Available simulators:"
            xcrun simctl list devices available | grep -i "iphone\|ipad" | head -10
            exit 1
        fi
    fi
    open -a Simulator

    if ! xcrun simctl install booted "$_PRODUCTS_DIR/$_PRODUCT_NAME.app" 2>&1; then
        _log_error "Failed to install app on simulator"
        _log_info "App path: $_PRODUCTS_DIR/$_PRODUCT_NAME.app"
        exit 1
    fi

    if ! xcrun simctl launch booted "$_BUNDLE_ID" 2>&1; then
        _log_error "Failed to launch app on simulator"
        _log_info "Bundle ID: $_BUNDLE_ID"
        exit 1
    fi

    _log_success "Launched: $_BUNDLE_ID on $simulator"
}

# --- Test ---
action_test() {
    local config="$XCODE_TOOLS_CONFIG"
    local scheme=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme) scheme="$2"; shift 2 ;;
            -c|--config) config="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    XCODE_TOOLS_CONFIG="$config"

    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"
    [[ -z "$scheme" ]] && scheme=$(_select_scheme)

    _log_step "Testing: ${scheme}"
    _run_cmd xcodebuild test "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" \
        -scheme "$scheme" -destination 'platform=macOS'
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        _log_error "FAILED: Test ${scheme}"
        _show_errors "Test"
        exit $exit_code
    fi
    _log_success "Tests passed: ${scheme}"
}

# --- Clean ---
action_clean() {
    local scheme=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme) scheme="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"
    [[ -z "$scheme" ]] && scheme=$(_select_scheme)

    _log_step "Cleaning: ${scheme}"
    xcodebuild clean "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" -scheme "$scheme"
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        _log_error "FAILED: Clean ${scheme}"
        exit $exit_code
    fi
    _log_success "Clean completed: ${scheme}"
}

# --- Stop Simulator App ---
action_stop_simulator() {
    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"
    local scheme=""
    [[ $# -gt 0 ]] && case $1 in -s|--scheme) scheme="$2" ;; esac
    [[ -z "$scheme" ]] && scheme=$(_select_scheme)

    _cache_build_settings "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" "$scheme"

    if [[ -n "$_BUNDLE_ID" ]]; then
        _log_step "Stopping: $_BUNDLE_ID"
        xcrun simctl terminate booted "$_BUNDLE_ID" 2>/dev/null && \
            _log_success "Stopped: $_BUNDLE_ID" || \
            _log_warn "App was not running"
    else
        _log_error "Could not determine bundle ID" >&2; exit 1
    fi
}

# --- Shutdown Simulator ---
action_shutdown_simulator() {
    _log_step "Shutting down all simulators"
    xcrun simctl shutdown all 2>/dev/null
    _log_success "All simulators shut down"
}

# --- List ---
action_list() {
    _detect_project
    _log_step "Available Schemes ($(basename "$_BUILD_TARGET"))"
    local schemes=()
    while IFS= read -r s; do [[ -n "$s" ]] && schemes+=("$s"); done < <(_discover_schemes)
    if [[ ${#schemes[@]} -eq 0 ]]; then
        _log_warn "No .xcscheme files found, trying xcodebuild -list..."
        while IFS= read -r s; do [[ -n "$s" ]] && schemes+=("$s"); done < <(_discover_schemes_fallback)
    fi
    if [[ ${#schemes[@]} -eq 0 ]]; then
        _log_error "No schemes found"
        _log_info "Open the project in Xcode once to generate schemes."
        return
    fi
    for i in "${!schemes[@]}"; do
        echo -e "  ${BOLD}$((i+1))${NC}) ${schemes[$i]}"
    done
}

# ============================================================================
# Main Dispatcher
# ============================================================================
main() {
    if [[ $# -eq 0 ]]; then
        echo "Usage: $(basename "$0") <action> [options]"
        echo ""
        echo "Actions:"
        echo "  build              Build the project"
        echo "  run-macos          Build & Run (macOS app)"
        echo "  run-simulator      Build & Run (iOS Simulator)"
        echo "  test               Run tests"
        echo "  clean              Clean build products"
        echo "  stop-simulator     Stop running simulator app"
        echo "  shutdown-simulator Shutdown all simulators"
        echo "  list               List available schemes"
        echo ""
        echo "Options:"
        echo "  -s, --scheme    Scheme name (or 'all')"
        echo "  -c, --config    Debug | Release (default: Debug)"
        echo "  -d, --device    Simulator name"
        echo "  --clean         Clean before building (build action only)"
        exit 0
    fi

    local action="$1"; shift

    case "$action" in
        build)              action_build "$@" ;;
        run-macos)          action_run_macos "$@" ;;
        run-simulator)      action_run_simulator "$@" ;;
        test)               action_test "$@" ;;
        clean)              action_clean "$@" ;;
        stop-simulator)     action_stop_simulator "$@" ;;
        shutdown-simulator) action_shutdown_simulator ;;
        list)               action_list ;;
        *)                  _log_error "Unknown action: $action"; exit 1 ;;
    esac
}

main "$@"
