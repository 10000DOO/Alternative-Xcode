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

# Scheme discovery scoped to a single .xcodeproj/.xcworkspace bundle (recursive BSP setup).
# Kept bundle-scoped (not directory-wide) so a sibling tester project's schemes do not leak in.
_discover_schemes_at() {
    local bundle="$1"
    {
        find "$bundle" -path "*/xcshareddata/xcschemes/*.xcscheme" 2>/dev/null
        find "$bundle" -path "*/xcuserdata/*/xcschemes/*.xcscheme" 2>/dev/null
    } | while IFS= read -r f; do basename "$f" .xcscheme; done | sort -u
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
_BUILD_START=0

_run_cmd() {
    _BUILD_START=$SECONDS
    local exit_code=0
    # pipefail + set -e 는 파이프라인 실패 시 PIPESTATUS 캡처 전에 스크립트를 죽여
    # _show_errors 호출이 건너뛰어진다. 파이프라인 구간만 일시적으로 off.
    set +e
    if command -v xcbeautify &>/dev/null; then
        "$@" 2>&1 | tee "$_LAST_LOG" | xcbeautify
        exit_code=${PIPESTATUS[0]}
    else
        # xcbeautify 없을 때: CompileSwift/Ld/WriteAuxiliaryFile 등 진행 라인을 제거하고
        # 에러·경고·최종 요약 라인만 표시
        "$@" 2>&1 | tee "$_LAST_LOG" | \
            grep -E "(: error:|: fatal error:|: warning:|: note:|error generated\.|^\*\* BUILD|^=== BUILD TARGET)" | \
            grep -v "^warning:.*was built for newer macOS version"
        exit_code=${PIPESTATUS[0]}
    fi
    set -e
    return $exit_code
}

_show_errors() {
    local context="$1"
    [[ ! -f "$_LAST_LOG" ]] && return

    local error_lines error_count warn_count
    error_lines=$(grep -E ": (error|fatal error):" "$_LAST_LOG" \
        | grep -v "^Command " | grep -v "^CompileSwift" | grep -v "^note:")
    error_count=$(echo "$error_lines" | grep -c . 2>/dev/null || echo "0")
    [[ -z "$error_lines" ]] && error_count=0
    warn_count=$(grep -cE ": warning:" "$_LAST_LOG" 2>/dev/null || echo "0")

    echo ""
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${RED}  BUILD FAILED${NC} — ${context}"
    local summary=""
    [[ "$error_count" -gt 0 ]] && summary+="${RED}${error_count} error(s)${NC}  "
    [[ "$warn_count"  -gt 0 ]] && summary+="${YELLOW}${warn_count} warning(s)${NC}"
    [[ -n "$summary" ]] && echo -e "  ${summary}"
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

    if [[ "$error_count" -gt 0 ]]; then
        echo ""
        local shown=0
        while IFS= read -r line; do
            [[ -z "$line" ]] && continue
            [[ $shown -ge 5 ]] && break

            # 파싱: /abs/path/to/file.swift:10:20: error: message
            local location msg
            location=$(echo "$line" | grep -oE "^[^:]+:[0-9]+:[0-9]+" || true)
            if [[ -n "$location" ]]; then
                local rel_loc
                rel_loc=$(echo "$location" | sed "s|^${SCRIPT_DIR}/||")
                msg=$(echo "$line" | sed 's/.*: fatal error: //; s/.*: error: //')
                echo -e "  ${RED}✗${NC} ${BOLD}${rel_loc}${NC}"
                echo -e "    ${msg}"
            else
                echo -e "  ${RED}✗${NC} ${line}"
            fi

            # Clang/ObjC 에러의 바로 다음 2줄은 소스 코드 + 캐럿('^')인 경우가 많다.
            # 에러 라인 번호를 찾아 다음 2줄을 컨텍스트로 표시. note: 라인이면 별도로 출력.
            local line_num
            line_num=$(grep -nF "$line" "$_LAST_LOG" 2>/dev/null | head -1 | cut -d: -f1)
            if [[ -n "$line_num" ]]; then
                local ctx1 ctx2
                ctx1=$(sed -n "$((line_num+1))p" "$_LAST_LOG")
                ctx2=$(sed -n "$((line_num+2))p" "$_LAST_LOG")

                # 첫 번째 컨텍스트 라인: note: 면 note 스타일, 아니면 소스 코드로 간주
                if [[ -n "$ctx1" ]]; then
                    if echo "$ctx1" | grep -qE ": note:"; then
                        local note_msg
                        note_msg=$(echo "$ctx1" | sed 's/.*: note: //')
                        echo -e "    ${CYAN}↳${NC} ${note_msg}"
                    else
                        echo -e "    ${CYAN}│${NC} ${ctx1}"
                    fi
                fi
                # 두 번째 컨텍스트 라인: note: / 소스 코드 / 캐럿('^') 모두 커버
                if [[ -n "$ctx2" ]]; then
                    if echo "$ctx2" | grep -qE ": note:"; then
                        local note_msg
                        note_msg=$(echo "$ctx2" | sed 's/.*: note: //')
                        echo -e "    ${CYAN}↳${NC} ${note_msg}"
                    else
                        echo -e "    ${CYAN}│${NC} ${ctx2}"
                    fi
                fi
            fi
            echo ""
            (( shown++ )) || true
        done <<< "$error_lines"

        if [[ "$error_count" -gt 5 ]]; then
            echo -e "  ${YELLOW}... 및 $((error_count - 5))개 에러 더 있음${NC}"
            echo ""
        fi
    fi

    echo -e "${BLUE}[INFO]${NC} 전체 로그: ${_LAST_LOG}"
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

    local elapsed=$(( SECONDS - _BUILD_START ))
    if [[ $exit_code -eq 0 ]]; then
        _log_success "SUCCEEDED: ${scheme} (${config}) — ${elapsed}s"
        _cache_build_settings "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" "$scheme"
        if [[ -n "$_PRODUCTS_DIR" ]] && [[ -d "$_PRODUCTS_DIR" ]]; then
            _log_info "Products: $_PRODUCTS_DIR"
            open "$_PRODUCTS_DIR"
        fi
    else
        _log_error "FAILED: ${scheme} (${config}) — ${elapsed}s"
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
    local test_class=""
    local test_func=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme) scheme="$2"; shift 2 ;;
            -c|--config) config="$2"; shift 2 ;;
            --test-class) test_class="$2"; shift 2 ;;
            --test-func) test_func="$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    XCODE_TOOLS_CONFIG="$config"

    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"
    [[ -z "$scheme" ]] && scheme=$(_select_scheme)

    local only_testing_args=()
    if [[ -n "$test_class" ]] || [[ -n "$test_func" ]]; then
        local test_target="${XCODE_TOOLS_TEST_TARGET:-${scheme}Tests}"
        if [[ -n "$test_class" ]] && [[ -n "$test_func" ]]; then
            only_testing_args=(-only-testing:"${test_target}/${test_class}/${test_func}")
            _log_step "Testing Function: ${test_class}.${test_func}"
        elif [[ -n "$test_class" ]]; then
            only_testing_args=(-only-testing:"${test_target}/${test_class}")
            _log_step "Testing Class: ${test_class}"
        elif [[ -n "$test_func" ]]; then
            only_testing_args=(-only-testing:"${test_target}/${test_func}")
            _log_step "Testing Bare Function: ${test_func}"
        fi
    else
        _log_step "Testing: ${scheme}"
    fi

    _run_cmd xcodebuild test "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" \
        -scheme "$scheme" -destination 'platform=macOS' \
        "${only_testing_args[@]}"
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        _log_error "FAILED: Test ${scheme}"
        _show_errors "Test"
        exit $exit_code
    fi
    _log_success "Tests passed: ${scheme}"
}

# --- Inline Test (Smart Wrapper) ---
action_inline_test() {
    local has_xcode=false
    if find . -maxdepth 2 -name "*.xcodeproj" -o -name "*.xcworkspace" | grep -q .; then
        has_xcode=true
    fi

    if [[ "$has_xcode" == "true" ]]; then
        _log_info "Detected Xcode project, using xcodebuild..."
        action_test "$@"
    else
        _log_info "No Xcode project detected, falling back to swift test..."
        local class="" func=""
        while [[ $# -gt 0 ]]; do
            case $1 in
                --test-class) class="$2"; shift 2 ;;
                --test-func) func="$2"; shift 2 ;;
                *) shift ;;
            esac
        done
        
        local filter=""
        if [[ -n "$class" ]] && [[ -n "$func" ]]; then
            filter="^\\w+\\.$class/$func\\b"
        elif [[ -n "$class" ]]; then
            filter="^\\w+\\.$class/"
        elif [[ -n "$func" ]]; then
            filter="^\\w+\\.$func\\b"
        fi
        
        if [[ -n "$filter" ]]; then
            _run_cmd swift test --filter "$filter"
        else
            _run_cmd swift test
        fi
    fi
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

# --- .gitignore 등록 (중복 검사 후 append) ---
_ensure_gitignore_at() {
    local dir="$1"
    local pattern="$2"
    local gitignore="$dir/.gitignore"

    if [[ ! -f "$gitignore" ]]; then
        echo "$pattern" > "$gitignore"
        _log_success ".gitignore created with: $pattern"
        return
    fi

    if grep -qxF "$pattern" "$gitignore"; then
        _log_info ".gitignore already ignores: $pattern"
    else
        # 파일이 개행으로 끝나지 않으면 먼저 개행을 넣어 이전 항목과 병합되지 않게 함
        [[ -n "$(tail -c1 "$gitignore")" ]] && printf '\n' >> "$gitignore"
        echo "$pattern" >> "$gitignore"
        _log_success ".gitignore updated with: $pattern"
    fi
}

_ensure_gitignore() {
    _ensure_gitignore_at "$SCRIPT_DIR" "$1"
}

# --- BSP prerequisites (provider binary + python3), shared by both modes ---
# Sets the global _BSP_BIN; exits on failure.
_BSP_BIN=""
_bsp_require_tools() {
    _BSP_BIN=""
    if [[ -x "$HOME/.config/zed/xcode-tools/bin/xcode-bsp" ]]; then
        _BSP_BIN="$HOME/.config/zed/xcode-tools/bin/xcode-bsp"
    elif command -v xcode-bsp &>/dev/null; then
        _BSP_BIN="$(command -v xcode-bsp)"
    fi

    if [[ -z "$_BSP_BIN" ]]; then
        _log_error "xcode-bsp provider binary not found"
        _log_info "Build & install it first: bash scripts/setup.sh (requires Rust/cargo)"
        exit 1
    fi

    if ! command -v python3 &>/dev/null; then
        _log_error "python3 not found (required to generate buildServer.json)"
        _log_info "Install the Xcode Command Line Tools: xcode-select --install"
        exit 1
    fi
}

# --- Write one buildServer.json into <out_dir> (returns non-zero on failure) ---
# Args: <bsp_bin> <project|workspace path> <flag> <scheme (may be empty)> <out_dir>
_write_build_server() {
    local bsp_bin="$1"
    local project="$2"
    local flag="$3"
    local sch="$4"
    local out_dir="$5"
    local out="$out_dir/buildServer.json"

    # Serialize with python3 for safe JSON escaping (paths may contain spaces).
    # Values are passed via env vars so nothing is interpolated into the source.
    # Field names (project/project_flag/scheme) must match bsp-server state.rs.
    if ! BSP_BIN="$bsp_bin" BSP_PROJECT="$project" BSP_FLAG="$flag" BSP_SCHEME="$sch" \
        python3 - "$out" <<'PYEOF'
import json, os, sys

doc = {
    "name": "xcode-tools bsp",
    "version": "0.1.0",
    "bspVersion": "2.2.0",
    "languages": ["c", "cpp", "objective-c", "objective-cpp", "swift"],
    "argv": [os.environ["BSP_BIN"]],
    # Canonical path so the server's srcroot (= project's parent) points at the
    # real source tree even if the workspace reaches the project via a symlink.
    "project": os.path.realpath(os.environ["BSP_PROJECT"]),
    "project_flag": os.environ["BSP_FLAG"],
}
scheme = os.environ.get("BSP_SCHEME", "")
if scheme:
    doc["scheme"] = scheme

# Write to a temp file then atomically rename so a mid-write failure never
# leaves the project's buildServer.json 0-byte/partial.
out = sys.argv[1]
tmp = out + ".tmp"
with open(tmp, "w") as f:
    json.dump(doc, f, indent=2)
    f.write("\n")
os.replace(tmp, out)
PYEOF
    then
        return 1
    fi

    [[ -s "$out" ]] || return 1
    return 0
}

# --- Recursive BSP setup: one buildServer.json per discovered project/workspace ---
# Used when the cwd has no directly-detectable project (e.g. a parent folder holding
# many sub-projects). Continues past individual failures; exits 0 if nothing failed
# or at least one target was created.
_bsp_setup_recursive() {
    local bsp_bin="$1"
    local maxdepth=5

    _log_step "Scanning for Xcode projects under $SCRIPT_DIR (recursive, depth $maxdepth)"

    # 1) Standalone workspaces (ignore project.xcworkspace inside .xcodeproj bundles).
    local workspaces=()
    while IFS= read -r -d '' ws; do
        workspaces+=("$ws")
    done < <(find "$SCRIPT_DIR" -maxdepth "$maxdepth" -name "*.xcworkspace" ! -path "*.xcodeproj/*" -print0 2>/dev/null)

    # Workspace directories: any .xcodeproj physically under one is treated as a
    # workspace member and excluded from standalone-project processing (dedupe).
    local ws_dirs=""
    if [[ ${#workspaces[@]} -gt 0 ]]; then
        local ws
        for ws in "${workspaces[@]}"; do
            ws_dirs+="$(dirname "$ws")"$'\n'
        done
    fi

    # 2) Standalone .xcodeproj (not nested inside another .xcodeproj bundle),
    #    excluding those that live under a workspace directory.
    local projects=()
    local pj
    while IFS= read -r -d '' pj; do
        local skip=false
        local wd
        while IFS= read -r wd; do
            [[ -z "$wd" ]] && continue
            if [[ "$pj" == "$wd/"* ]]; then skip=true; break; fi
        done <<< "$ws_dirs"
        [[ "$skip" == "true" ]] && continue
        projects+=("$pj")
    done < <(find "$SCRIPT_DIR" -maxdepth "$maxdepth" -name "*.xcodeproj" ! -path "*.xcodeproj/*" -print0 2>/dev/null)

    local total=0 created=0 skipped=0 failed=0

    # --- Workspaces: scheme omitted (server enumerates all member projects). ---
    if [[ ${#workspaces[@]} -gt 0 ]]; then
        for ws in "${workspaces[@]}"; do
            total=$((total + 1))
            local outdir; outdir="$(dirname "$ws")"
            local name; name="$(basename "$outdir")"
            if [[ -e "$outdir/buildServer.json" ]]; then
                _log_info "SKIP  $name — buildServer.json already exists"
                skipped=$((skipped + 1)); continue
            fi
            if _write_build_server "$bsp_bin" "$ws" "-workspace" "" "$outdir"; then
                _ensure_gitignore_at "$outdir" "buildServer.json"
                _log_success "OK    $name (workspace)"
                created=$((created + 1))
            else
                _log_error "FAIL  $name (workspace) — could not write buildServer.json"
                failed=$((failed + 1))
            fi
        done
    fi

    # --- Standalone projects: pick the main scheme by directory-name match. ---
    if [[ ${#projects[@]} -gt 0 ]]; then
        for pj in "${projects[@]}"; do
            total=$((total + 1))
            local outdir; outdir="$(dirname "$pj")"
            local name; name="$(basename "$outdir")"
            if [[ -e "$outdir/buildServer.json" ]]; then
                _log_info "SKIP  $name — buildServer.json already exists"
                skipped=$((skipped + 1)); continue
            fi

            local schemes=()
            while IFS= read -r s; do [[ -n "$s" ]] && schemes+=("$s"); done < <(_discover_schemes_at "$pj")

            local chosen=""
            if [[ ${#schemes[@]} -eq 1 ]]; then
                chosen="${schemes[0]}"
            elif [[ ${#schemes[@]} -gt 1 ]]; then
                # Main scheme = the one whose name equals the directory basename
                # (case-insensitive). Verified 100% accurate across the sdks tree.
                local target_lc; target_lc="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
                local sc sc_lc
                for sc in "${schemes[@]}"; do
                    sc_lc="$(printf '%s' "$sc" | tr '[:upper:]' '[:lower:]')"
                    if [[ "$sc_lc" == "$target_lc" ]]; then chosen="$sc"; break; fi
                done
                if [[ -z "$chosen" ]]; then
                    _log_warn "SKIP  $name — multiple schemes, none match '$name': ${schemes[*]}"
                    skipped=$((skipped + 1)); continue
                fi
            fi
            # 0 schemes → chosen stays empty (omit -scheme, bind latest build scheme).

            if _write_build_server "$bsp_bin" "$pj" "-project" "$chosen" "$outdir"; then
                _ensure_gitignore_at "$outdir" "buildServer.json"
                if [[ -n "$chosen" ]]; then
                    _log_success "OK    $name (scheme: $chosen)"
                else
                    _log_success "OK    $name (no scheme — bind latest)"
                fi
                created=$((created + 1))
            else
                _log_error "FAIL  $name — could not write buildServer.json"
                failed=$((failed + 1))
            fi
        done
    fi

    _log_step "BSP setup summary"
    _log_info "Targets: $total   Created: $created   Skipped: $skipped   Failed: $failed"

    if [[ $total -eq 0 ]]; then
        _log_error "No .xcworkspace or .xcodeproj found under $SCRIPT_DIR (depth $maxdepth)"
        exit 1
    fi
    if [[ $created -gt 0 ]]; then
        _log_info "Provider: $bsp_bin"
        _log_info "Reload/restart Zed so SourceKit-LSP picks up the new buildServer.json files."
    fi
    # Fail only when nothing succeeded and at least one target errored.
    if [[ $created -eq 0 && $failed -gt 0 ]]; then
        exit 1
    fi
    exit 0
}

# --- BSP Setup (writes buildServer.json pointing at our xcode-bsp provider) ---
action_bsp_setup() {
    local scheme=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            -s|--scheme) scheme="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    # Prerequisites shared by single-target and recursive modes.
    _bsp_require_tools

    # Mode select: if a project/workspace is directly detectable (same criteria as
    # _detect_project — a maxdepth-1 workspace or a maxdepth-2 project), keep the
    # existing single-target behavior. Otherwise recurse into sub-directories so a
    # parent folder holding many sub-projects can be set up in one pass.
    local local_ws local_pj
    local_ws="$(find "$SCRIPT_DIR" -maxdepth 1 -name "*.xcworkspace" ! -path "*.xcodeproj/*" 2>/dev/null || true)"
    local_pj="$(find "$SCRIPT_DIR" -maxdepth 2 -name "*.xcodeproj" 2>/dev/null || true)"

    if [[ -z "$local_ws" && -z "$local_pj" ]]; then
        _bsp_setup_recursive "$_BSP_BIN"
        return
    fi

    # ── Single-target mode (unchanged behavior) ──
    _detect_project
    _log_info "Project: $(basename "$_BUILD_TARGET")"

    # 스킴 비대화식 해석 (Zed Task는 read 프롬프트 불가)
    # Workspaces: the server enumerates ALL projects/targets and routes per file,
    # so no scheme is needed — the single-project fail-safe must NOT apply here.
    if [[ "$_BUILD_TARGET_FLAG" == "-workspace" ]]; then
        _log_info "Workspace detected — enumerating all projects/targets (scheme omitted)."
    elif [[ -z "$scheme" ]]; then
        local schemes=()
        while IFS= read -r s; do [[ -n "$s" ]] && schemes+=("$s"); done < <(_discover_schemes)
        if [[ ${#schemes[@]} -eq 0 ]]; then
            while IFS= read -r s; do [[ -n "$s" ]] && schemes+=("$s"); done < <(_discover_schemes_fallback)
        fi

        if [[ ${#schemes[@]} -eq 1 ]]; then
            scheme="${schemes[0]}"
            _log_info "Scheme: $scheme"
        elif [[ ${#schemes[@]} -gt 1 ]]; then
            _log_error "Multiple schemes found: ${schemes[*]}"
            _log_error "Refusing to guess the LSP build context. Re-run with an explicit scheme:"
            _log_error "  bsp-setup -s <scheme>"
            exit 1
        else
            _log_warn "No schemes found — binding latest build scheme (omitting -scheme)"
        fi
    else
        _log_info "Scheme: $scheme"
    fi

    _log_step "Generating buildServer.json"
    if ! _write_build_server "$_BSP_BIN" "$_BUILD_TARGET" "$_BUILD_TARGET_FLAG" "$scheme" "$SCRIPT_DIR"; then
        _log_error "Failed to write buildServer.json in $SCRIPT_DIR"
        exit 1
    fi

    _ensure_gitignore "buildServer.json"
    _log_success "buildServer.json generated in $SCRIPT_DIR"
    _log_info "Provider: $_BSP_BIN"
    _log_info "Reload/restart Zed so SourceKit-LSP picks up buildServer.json."
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
        echo "  bsp-setup          Set up SourceKit-LSP build context (buildServer.json)"
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
        inline-test)        action_inline_test "$@" ;;
        clean)              action_clean "$@" ;;
        stop-simulator)     action_stop_simulator "$@" ;;
        shutdown-simulator) action_shutdown_simulator ;;
        list)               action_list ;;
        bsp-setup)          action_bsp_setup "$@" ;;
        *)                  _log_error "Unknown action: $action"; exit 1 ;;
    esac
}

main "$@"
