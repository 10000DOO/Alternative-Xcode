# Design: Inline Test Runner (runnables.scm Integration)

## 1. Goal
Enable inline test execution in the Zed editor—displaying a play (▶) button next to test classes and functions—allowing users to run a specific test instead of the entire suite.

## 2. Requirements & Constraints
- **Grammar Constraint**: Zed allows only one active `runnables.scm` per language.
- **Existing Ecosystem**: The official `zed-extensions/swift` already provides a `runnables.scm` that parses Swift tests and emits specific tags:
  - `swift-xctest-class`, `swift-xctest-func`
  - `swift-testing-suite`, `swift-testing-member-func`, `swift-testing-bare-func`
- **Environment Variables**: The official extension also provides `$ZED_CUSTOM_SWIFT_TEST_CLASS` and `$ZED_CUSTOM_SWIFT_TEST_FUNC` to the task execution environment.

**Architectural Decision**: To avoid conflicts and prevent over-engineering, we will **not** create a custom `runnables.scm`. Instead, we will hook into the official extension's tags by updating our `languages/swift/tasks.json` to listen for these tags, and extend `scripts/helpers.sh` to handle the specific test execution.

## 3. Architecture Overview & Interface Contract

### 3.1 `languages/swift/tasks.json`
We will add two new tasks to our `tasks.json`. These tasks will use the `tags` property to bind to the play button UI provided by the official extension's grammar.

```json
[
  {
    "label": "Xcode: Test Class ($ZED_CUSTOM_SWIFT_TEST_CLASS)",
    "command": "bash",
    "args": [
      "-c",
      "source \"$HOME/Library/Application Support/Zed/extensions/installed/xcode-tools/scripts/helpers.sh\" && action_test --test-class \"$ZED_CUSTOM_SWIFT_TEST_CLASS\""
    ],
    "cwd": "$ZED_WORKTREE_ROOT",
    "tags": ["swift-xctest-class", "swift-testing-suite"]
  },
  {
    "label": "Xcode: Test Function ($ZED_CUSTOM_SWIFT_TEST_FUNC)",
    "command": "bash",
    "args": [
      "-c",
      "source \"$HOME/Library/Application Support/Zed/extensions/installed/xcode-tools/scripts/helpers.sh\" && action_test --test-class \"$ZED_CUSTOM_SWIFT_TEST_CLASS\" --test-func \"$ZED_CUSTOM_SWIFT_TEST_FUNC\""
    ],
    "cwd": "$ZED_WORKTREE_ROOT",
    "tags": ["swift-xctest-func", "swift-testing-member-func", "swift-testing-bare-func"]
  }
]
```
*(Note: To ensure the `helpers.sh` script is located correctly, we source it from the standard Zed extension installation path. If the extension provides a binary wrapper in the future, this can be simplified).*

### 3.2 `scripts/helpers.sh`
The `action_test` function will be updated to accept `--test-class` and `--test-func` flags. It will map these to `xcodebuild`'s `-only-testing` flag.

**Format**: `xcodebuild test -only-testing:<TestTarget>/<TestClass>/<TestMethod>`

Since the test target name is not provided by the syntax tree, we will infer it by appending `Tests` to the current scheme name (a standard Xcode convention). We will also expose an environment variable `XCODE_TOOLS_TEST_TARGET` for users who use custom naming conventions.

```bash
# Inside action_test() parsing loop:
--test-class) test_class="$2"; shift 2 ;;
--test-func)  test_func="$2"; shift 2 ;;

# Constructing the flag:
local only_testing_args=()
if [[ -n "$test_class" ]]; then
    local test_target="${XCODE_TOOLS_TEST_TARGET:-${scheme}Tests}"
    if [[ -n "$test_func" ]]; then
        only_testing_args=(-only-testing:"${test_target}/${test_class}/${test_func}")
        _log_step "Testing Function: ${test_class}.${test_func}"
    else
        only_testing_args=(-only-testing:"${test_target}/${test_class}")
        _log_step "Testing Class: ${test_class}"
    fi
fi

# Appending to xcodebuild:
_run_cmd xcodebuild test "$_BUILD_TARGET_FLAG" "$_BUILD_TARGET" \
    -scheme "$scheme" -destination 'platform=macOS' \
    "${only_testing_args[@]}"
```

## 4. Impact Analysis
- **Files Modified**: 
  - `languages/swift/tasks.json`
  - `scripts/helpers.sh`
- **Dependencies**: Relies on `zed-extensions/swift` being installed and active for the tags to be emitted. This is a safe assumption for Swift development in Zed.
- **Backward Compatibility**: Calling `helpers.sh test` without the new flags will continue to run the entire test suite, preserving existing behavior.

## 5. Migration Steps
1. **Update `helpers.sh`**: Modify `action_test` to parse the new arguments and conditionally append the `-only-testing` array to the `xcodebuild test` command.
2. **Update `tasks.json`**: Append the two new inline test tasks with the appropriate `tags`.
3. **Validation**: 
   - Open a `*Tests.swift` file in Zed.
   - Verify the ▶ button appears next to a test class.
   - Click the button and verify the terminal executes only that specific class.

## 6. Checklist
- [ ] Define `--test-class` and `--test-func` parsing in `helpers.sh`.
- [ ] Implement `-only-testing` flag construction using inferred target name.
- [ ] Add `XCODE_TOOLS_TEST_TARGET` environment variable support for edge cases.
- [ ] Add `swift-xctest-*` and `swift-testing-*` tasks to `tasks.json`.
- [ ] Ensure full test suite runs still work correctly when no flags are passed.
