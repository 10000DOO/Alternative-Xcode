use zed_extension_api as zed;
use serde::{Deserialize, Serialize};

const ADAPTER_NAME: &str = "xcode-debug";

// ── User config parsed from .zed/debug.json ──
// Fields match lldb-dap's launch/attach request format directly.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct XcodeDebugConfig {
    request: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_on_entry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wait_for: Option<bool>,
}

struct XcodeToolsExtension;

// ── DAP binary resolution (independent function for v0.2 modularization) ──

fn resolve_dap_binary(
    user_path: Option<String>,
    worktree: &zed::Worktree,
) -> Result<(String, Vec<String>), String> {
    // 1. User-provided path takes priority
    if let Some(path) = user_path {
        return Ok((path, vec![]));
    }
    // 2. xcrun lldb-dap (Xcode built-in, most stable)
    if worktree.which("xcrun").is_some() {
        return Ok(("xcrun".to_string(), vec!["lldb-dap".to_string()]));
    }
    // 3. Bare lldb-dap on PATH
    if let Some(path) = worktree.which("lldb-dap") {
        return Ok((path, vec![]));
    }
    Err("lldb-dap not found. Install Xcode Command Line Tools or set a custom debug adapter path.".to_string())
}

// ── Config validation (independent to keep get_dap_binary thin and unit-testable) ──

fn validate_config(config: &XcodeDebugConfig) -> Result<(), String> {
    match config.request.as_str() {
        "launch" => {
            let program = config.program.as_deref().unwrap_or("");
            if program.is_empty() {
                return Err(
                    "xcode-debug: 'program' is required for launch. \
                     Set it in .zed/debug.json, e.g. \
                     \"program\": \"${ZED_WORKTREE_ROOT}/build/Debug/MyApp.app/Contents/MacOS/MyApp\""
                        .to_string(),
                );
            }
            Ok(())
        }
        "attach" => {
            let has_pid = config.pid.is_some();
            let has_program = config.program.as_deref().unwrap_or("").is_empty() == false;
            let waits = config.wait_for == Some(true);

            if !has_pid && !has_program {
                return Err(
                    "xcode-debug: attach requires either 'pid' (integer process ID) \
                     or 'program' (executable path or name)."
                        .to_string(),
                );
            }
            if has_pid && waits {
                return Err(
                    "xcode-debug: 'waitFor: true' can only be used when attaching by 'program', not 'pid'."
                        .to_string(),
                );
            }
            Ok(())
        }
        other => Err(format!(
            "xcode-debug: invalid 'request' value: {other} (expected \"launch\" or \"attach\")"
        )),
    }
}

// ── cwd resolution (per architecture.md §3: parsed cwd > worktree root fallback) ──

fn resolve_cwd(parsed_cwd: Option<&str>, worktree_root: &str) -> String {
    parsed_cwd
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| worktree_root.to_string())
}

// ── Build DAP configuration JSON (independent function for reuse) ──

fn build_dap_configuration(config: &XcodeDebugConfig) -> Result<String, String> {
    let mut value = serde_json::to_value(config)
        .map_err(|e| format!("Serialize error: {e}"))?;
    // lldb-dap receives request type via DAP protocol, not inside configuration JSON
    if let Some(obj) = value.as_object_mut() {
        obj.remove("request");
    }
    serde_json::to_string(&value).map_err(|e| format!("Serialize error: {e}"))
}

// ── Extension trait implementation ──

impl zed::Extension for XcodeToolsExtension {
    fn new() -> Self {
        XcodeToolsExtension
    }

    fn get_dap_binary(
        &mut self,
        adapter_name: String,
        config: zed::DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        worktree: &zed::Worktree,
    ) -> Result<zed::DebugAdapterBinary, String> {
        if adapter_name != ADAPTER_NAME {
            return Err(format!("Unknown adapter: {adapter_name}"));
        }

        let parsed: XcodeDebugConfig = serde_json::from_str(&config.config)
            .map_err(|e| format!("Config parse error: {e}"))?;

        validate_config(&parsed)?;

        let request = match parsed.request.as_str() {
            "launch" => zed::StartDebuggingRequestArgumentsRequest::Launch,
            "attach" => zed::StartDebuggingRequestArgumentsRequest::Attach,
            other => return Err(format!("Invalid request type: {other}")),
        };

        let (command, arguments) =
            resolve_dap_binary(user_provided_debug_adapter_path, worktree)?;

        let configuration = build_dap_configuration(&parsed)?;

        Ok(zed::DebugAdapterBinary {
            command: Some(command),
            arguments,
            envs: vec![],
            cwd: Some(resolve_cwd(parsed.cwd.as_deref(), &worktree.root_path())),
            connection: None,
            request_args: zed::StartDebuggingRequestArguments {
                configuration,
                request,
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        config: serde_json::Value,
    ) -> Result<zed::StartDebuggingRequestArgumentsRequest, String> {
        match config.get("request").and_then(|v| v.as_str()) {
            Some("launch") => Ok(zed::StartDebuggingRequestArgumentsRequest::Launch),
            Some("attach") => Ok(zed::StartDebuggingRequestArgumentsRequest::Attach),
            Some(other) => Err(format!("Unknown request type: {other}")),
            None => Err("Missing 'request' field in debug configuration".to_string()),
        }
    }

    fn dap_config_to_scenario(
        &mut self,
        config: zed::DebugConfig,
    ) -> Result<zed::DebugScenario, String> {
        let debug_config = match &config.request {
            zed::DebugRequest::Launch(launch) => {
                let env: Vec<String> = launch
                    .envs
                    .iter()
                    .map(|(k, v)| {
                        if v.is_empty() {
                            k.clone()
                        } else {
                            format!("{k}={v}")
                        }
                    })
                    .collect();

                XcodeDebugConfig {
                    request: "launch".to_string(),
                    program: Some(launch.program.clone()),
                    cwd: launch.cwd.clone(),
                    args: launch.args.clone(),
                    env,
                    pid: None,
                    stop_on_entry: config.stop_on_entry,
                    wait_for: None,
                }
            }
            zed::DebugRequest::Attach(attach) => XcodeDebugConfig {
                request: "attach".to_string(),
                program: None,
                cwd: None,
                args: vec![],
                env: vec![],
                pid: attach.process_id,
                stop_on_entry: config.stop_on_entry,
                wait_for: None,
            },
        };

        let config_json = serde_json::to_string(&debug_config)
            .map_err(|e| format!("Serialize error: {e}"))?;

        Ok(zed::DebugScenario {
            label: config.label,
            adapter: ADAPTER_NAME.to_string(),
            build: None,
            config: config_json,
            tcp_connection: None,
        })
    }
}

zed::register_extension!(XcodeToolsExtension);

#[cfg(test)]
mod tests {
    use super::*;

    // ── XcodeDebugConfig parsing ──

    #[test]
    fn parse_launch_config() {
        let json = r#"{
            "request": "launch",
            "program": "/tmp/a.out",
            "args": ["--verbose"],
            "env": ["FOO=1", "BAR"],
            "stopOnEntry": true
        }"#;
        let config: XcodeDebugConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.request, "launch");
        assert_eq!(config.program.as_deref(), Some("/tmp/a.out"));
        assert_eq!(config.args, vec!["--verbose"]);
        assert_eq!(config.env, vec!["FOO=1", "BAR"]);
        assert_eq!(config.stop_on_entry, Some(true));
        assert_eq!(config.pid, None);
    }

    #[test]
    fn parse_attach_config() {
        let json = r#"{
            "request": "attach",
            "pid": 12345,
            "waitFor": true
        }"#;
        let config: XcodeDebugConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.request, "attach");
        assert_eq!(config.pid, Some(12345));
        assert_eq!(config.wait_for, Some(true));
        assert_eq!(config.program, None);
    }

    #[test]
    fn parse_minimal_config() {
        let json = r#"{"request": "launch", "program": "/app"}"#;
        let config: XcodeDebugConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.args, Vec::<String>::new());
        assert_eq!(config.env, Vec::<String>::new());
        assert_eq!(config.cwd, None);
        assert_eq!(config.stop_on_entry, None);
        assert_eq!(config.wait_for, None);
    }

    // ── build_dap_configuration ──

    #[test]
    fn dap_config_excludes_request() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: Some("/tmp/a.out".to_string()),
            cwd: None,
            args: vec![],
            env: vec!["PATH=/usr/bin".to_string()],
            pid: None,
            stop_on_entry: Some(false),
            wait_for: None,
        };
        let json = build_dap_configuration(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(value.get("request").is_none(), "request must be excluded");
        assert_eq!(value["program"], "/tmp/a.out");
        assert_eq!(value["env"][0], "PATH=/usr/bin");
    }

    #[test]
    fn dap_config_roundtrip_camel_case() {
        let config = XcodeDebugConfig {
            request: "attach".to_string(),
            program: None,
            cwd: None,
            args: vec![],
            env: vec![],
            pid: Some(999),
            stop_on_entry: Some(true),
            wait_for: Some(true),
        };
        let json = build_dap_configuration(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["pid"], 999);
        assert_eq!(value["stopOnEntry"], true);
        assert_eq!(value["waitFor"], true);
        // snake_case must NOT appear in output
        assert!(value.get("stop_on_entry").is_none());
        assert!(value.get("wait_for").is_none());
    }

    // ── Serialization format matches lldb-dap ──

    #[test]
    fn serialized_env_is_string_array() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: Some("/app".to_string()),
            cwd: None,
            args: vec![],
            env: vec!["A=1".to_string(), "B=2".to_string()],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        let json = build_dap_configuration(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(value["env"].is_array());
        assert_eq!(value["env"][0], "A=1");
        assert_eq!(value["env"][1], "B=2");
    }

    // ── A. skip_serializing_if: absent optional/empty fields drop from JSON ──

    #[test]
    fn launch_serialized_omits_pid_and_wait_for() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: Some("/tmp/a.out".to_string()),
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        let json = build_dap_configuration(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(value.get("pid").is_none());
        assert!(value.get("waitFor").is_none());
    }

    #[test]
    fn attach_serialized_omits_program_args_env() {
        let config = XcodeDebugConfig {
            request: "attach".to_string(),
            program: None,
            cwd: None,
            args: vec![],
            env: vec![],
            pid: Some(12345),
            stop_on_entry: None,
            wait_for: None,
        };
        let json = build_dap_configuration(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(value.get("program").is_none());
        assert!(value.get("args").is_none());
        assert!(value.get("env").is_none());
    }

    #[test]
    fn empty_args_and_env_are_omitted() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: Some("/app".to_string()),
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        let json = build_dap_configuration(&config).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(value.get("args").is_none());
        assert!(value.get("env").is_none());
    }

    // ── B. validate_config launch ──

    #[test]
    fn validate_launch_requires_program() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: None,
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_launch_rejects_empty_program() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: Some(String::new()),
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_launch_accepts_valid_program() {
        let config = XcodeDebugConfig {
            request: "launch".to_string(),
            program: Some("/tmp/a.out".to_string()),
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        assert!(validate_config(&config).is_ok());
    }

    // ── C. validate_config attach ──

    #[test]
    fn validate_attach_requires_pid_or_program() {
        let config = XcodeDebugConfig {
            request: "attach".to_string(),
            program: None,
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: None,
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_attach_accepts_pid() {
        let config = XcodeDebugConfig {
            request: "attach".to_string(),
            program: None,
            cwd: None,
            args: vec![],
            env: vec![],
            pid: Some(12345),
            stop_on_entry: None,
            wait_for: None,
        };
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_attach_accepts_program_and_wait_for_true() {
        let config = XcodeDebugConfig {
            request: "attach".to_string(),
            program: Some("MyApp".to_string()),
            cwd: None,
            args: vec![],
            env: vec![],
            pid: None,
            stop_on_entry: None,
            wait_for: Some(true),
        };
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_attach_rejects_pid_and_wait_for_true() {
        let config = XcodeDebugConfig {
            request: "attach".to_string(),
            program: None,
            cwd: None,
            args: vec![],
            env: vec![],
            pid: Some(12345),
            stop_on_entry: None,
            wait_for: Some(true),
        };
        assert!(validate_config(&config).is_err());
    }

    // ── D. resolve_cwd ──

    #[test]
    fn cwd_uses_parsed_when_present() {
        assert_eq!(resolve_cwd(Some("/custom"), "/root"), "/custom");
    }

    #[test]
    fn cwd_falls_back_to_worktree_root() {
        assert_eq!(resolve_cwd(None, "/root"), "/root");
    }

    #[test]
    fn cwd_empty_string_falls_back_to_worktree_root() {
        assert_eq!(resolve_cwd(Some(""), "/root"), "/root");
    }
}
