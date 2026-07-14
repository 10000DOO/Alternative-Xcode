//! xcode-bsp — native BSP server for SourceKit-LSP (Xcode projects).
//!
//! The wire protocol implemented here is specified in
//! `docs/bsp_protocol_spec.md` (§0 transport / framing, §9 Rust gotchas).
//!
//! Phase 3a-1: stdio JSON-RPC transport framing.
//! Phase 3a-2: no-build clang-arg synthesis library (`build_settings`,
//! `pbxproj`, `synth`) + a `synth` debug subcommand.
//! Phase 3a-3a: real BSP handlers (`handle_message` + `state::ServerState`),
//! making this a working SourceKit-LSP build server for the ObjC single-target
//! case. See `docs/bsp_protocol_spec.md` §1–§7, §9.

mod build_settings;
mod pbxproj;
mod runner;
mod state;
mod synth;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::state::ServerState;

/// How often the background watcher polls watched-file mtimes.
const WATCH_POLL: Duration = Duration::from_secs(2);

/// BSP language ids advertised by this server (spec §1/§3).
fn language_ids() -> Value {
    serde_json::json!(["c", "cpp", "objective-c", "objective-cpp", "swift"])
}

/// Upper bound on a single message body. A bogus huge `Content-Length` must not
/// trigger an unbounded zeroed allocation (which aborts via handle_alloc_error).
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

// ── Transport: read one LSP frame, returning the raw JSON body bytes ──
// Reads header lines until a blank line (must include `Content-Length:`; other
// headers like `Content-Type` are ignored), then exactly N bytes of body.
// Ok(None) on clean EOF at a frame boundary. Err ONLY on true framing/IO
// failure (stream position lost) — body JSON validity is the caller's concern
// (the N bytes are consumed either way, so a bad body is recoverable).
fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return if content_length.is_none() {
                Ok(None) // clean EOF between frames
            } else {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF within frame headers"))
            };
        }
        let line = line.trim_end(); // drop trailing \r\n
        if line.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            let n: usize = rest.trim().parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length value")
            })?;
            content_length = Some(n);
        }
        // Ignore any other header (Content-Type, etc.).
    }

    let len = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    if len > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Content-Length exceeds maximum",
        ));
    }

    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

// ── Transport: write one LSP-framed JSON-RPC message ──
fn write_message<W: Write>(writer: &mut W, msg: &Value) -> io::Result<()> {
    let body =
        serde_json::to_string(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // Content-Length is the UTF-8 byte length of the JSON body (spec §0 / §9).
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

// ── JSON-RPC response envelopes (id echoed; jsonrpc always "2.0") ──

fn ok_response(id: &Value, result: Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("jsonrpc".to_string(), Value::from("2.0"));
    m.insert("id".to_string(), id.clone());
    m.insert("result".to_string(), result);
    Value::Object(m)
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    let mut err = serde_json::Map::new();
    err.insert("code".to_string(), Value::from(code));
    err.insert("message".to_string(), Value::from(message));
    let mut m = serde_json::Map::new();
    m.insert("jsonrpc".to_string(), Value::from("2.0"));
    m.insert("id".to_string(), id.clone());
    m.insert("error".to_string(), Value::Object(err));
    Value::Object(m)
}

// ── URI helpers (SourceKit-LSP uses percent-encoded file:// URIs) ──

fn uri_to_path(uri: &str) -> String {
    match uri.strip_prefix("file://") {
        Some(rest) => percent_decode(rest),
        None => uri.to_string(),
    }
}

fn path_to_uri(path: &Path) -> String {
    // Percent-encode reserved/non-ASCII bytes (mirrors percent_decode); keep the
    // path separator and unreserved chars literal.
    let mut uri = String::from("file://");
    for &b in path.to_string_lossy().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ── Pure BSP dispatch (testable without stdio / subprocess) ──
// Returns Some(response) for requests, None for notifications. `build/exit`
// process termination is handled by the caller (run_stdio_loop).

fn handle_message(state: &mut ServerState, msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id");

    // Init order (spec §9.4): nothing but build/initialize before initialize.
    if !state.initialized && method != "build/initialize" {
        return id.map(|id| error_response(id, 123, "server not initialized"));
    }

    match method {
        "build/initialize" => {
            let id = id?;
            let root_uri = msg
                .get("params")
                .and_then(|p| p.get("rootUri"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let root_path = uri_to_path(&root_uri);
            *state = ServerState::init_from_root(Path::new(&root_path));
            // Best-effort: advertise the PRIMARY project's index store (one store
            // per multi-project workspace — a documented limitation).
            let index_store = state.primary_index_store();

            // Index advertisement (spec §1): only include paths we actually have.
            let mut data = serde_json::Map::new();
            data.insert("sourceKitOptionsProvider".to_string(), Value::Bool(true));
            if let Some(idx) = &index_store {
                data.insert(
                    "indexStorePath".to_string(),
                    Value::from(idx.to_string_lossy().into_owned()),
                );
            }
            if let Some(db) = state.index_database_path() {
                data.insert(
                    "indexDatabasePath".to_string(),
                    Value::from(db.to_string_lossy().into_owned()),
                );
            }

            let result = serde_json::json!({
                "displayName": "xcode-tools bsp",
                "version": env!("CARGO_PKG_VERSION"),
                "bspVersion": "2.2.0",
                "rootUri": root_uri,
                "capabilities": { "languageIds": language_ids() },
                "data": Value::Object(data),
                "dataKind": "sourceKit"
            });
            state.initialized = true;
            Some(ok_response(id, result))
        }

        // Notification: no file watching in 3a.
        "build/initialized" => None,

        "workspace/buildTargets" => {
            let id = id?;
            let result = serde_json::json!({
                "targets": [ {
                    "id": { "uri": "dummy://dummy" },
                    "displayName": "BuildServer",
                    "tags": ["test"],
                    "capabilities": {},
                    "languageIds": language_ids(),
                    "dependencies": []
                } ]
            });
            Some(ok_response(id, result))
        }

        "buildTarget/sources" => {
            let id = id?;
            let result = serde_json::json!({
                "items": [ {
                    "target": { "uri": "dummy://dummy" },
                    "sources": [ {
                        "uri": path_to_uri(&state.root),
                        "kind": 2,
                        "generated": false
                    } ]
                } ]
            });
            Some(ok_response(id, result))
        }

        // Both the old and prefixed names route here (spec §5, §9.2).
        "textDocument/sourceKitOptions" | "sourcekit/textDocument/sourceKitOptions" => {
            let id = id?;
            Some(handle_source_kit_options(state, msg, id))
        }

        "workspace/waitForBuildSystemUpdates"
        | "sourcekit/workspace/waitForBuildSystemUpdates" => {
            let id = id?;
            Some(ok_response(id, serde_json::json!({})))
        }

        // No-op: we don't prepare/build targets (spec §9.2).
        "buildTarget/prepare" | "sourcekit/buildTarget/prepare" => {
            let id = id?;
            Some(ok_response(id, Value::Null))
        }

        "build/shutdown" => {
            let id = id?;
            Some(ok_response(id, Value::Null))
        }

        // Process termination handled by the loop.
        "build/exit" => None,

        // Unknown: error for requests, silently ignore notifications (spec §9.5).
        _ => id.map(|id| error_response(id, 123, &format!("unhandled method {method}"))),
    }
}

// sourceKitOptions: synthesize C-family clang args; never a JSON-RPC error —
// unsupported/unknown cases return `result: null` (spec §5, §9.3).
fn handle_source_kit_options(state: &mut ServerState, msg: &Value, id: &Value) -> Value {
    let uri = msg
        .get("params")
        .and_then(|p| p.get("textDocument"))
        .and_then(|d| d.get("uri"))
        .and_then(Value::as_str);
    let file = match uri {
        Some(u) => PathBuf::from(uri_to_path(u)),
        None => return ok_response(id, Value::Null),
    };

    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const C_EXTS: &[&str] = &["c", "m", "mm", "cc", "cpp", "cxx", "h", "hpp"];
    let is_swift = ext == "swift";
    if !is_swift && !C_EXTS.contains(&ext.as_str()) {
        return ok_response(id, Value::Null);
    }

    // Layer (a): pick up a changed .pbxproj/.xcconfig/Package.resolved before
    // answering (no Zed restart needed). Cheap stat; re-enumerates only on change.
    state.refresh_if_stale();

    // Route the file to its owning target (per-file routing across projects).
    state.ensure_enumerated();
    let Some(key) = state.route(&file) else {
        return ok_response(id, Value::Null);
    };
    state.ensure_target(&key);

    let working_dir = state.workspace_root().to_string_lossy().into_owned();

    // Swift: whole-module args (identical for every .swift in the target).
    if is_swift {
        return match state.target_swift_args(&key) {
            Some(args) if !args.is_empty() => ok_response(
                id,
                serde_json::json!({
                    "compilerArguments": args.to_vec(),
                    "workingDirectory": working_dir
                }),
            ),
            _ => ok_response(id, Value::Null),
        };
    }

    // C-family: per-file clang args using the target's scoped include dirs.
    let Some(settings) = state.target_settings(&key) else {
        return ok_response(id, Value::Null);
    };
    let args = synth::synthesize_clang_args(settings, &file, state.target_includes(&key));
    if args.is_empty() {
        return ok_response(id, Value::Null);
    }
    let result = serde_json::json!({
        "compilerArguments": args,
        "workingDirectory": working_dir
    });
    ok_response(id, result)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().map(String::as_str) == Some("synth") {
        // Debug/aux command for verification; not the primary mode.
        run_synth_command(&args[1..])
    } else {
        run_stdio_loop().map_err(|e| e.to_string())
    };
    if let Err(msg) = result {
        eprintln!("[xcode-bsp] error: {msg}");
        std::process::exit(1);
    }
}

// ── Primary mode: BSP stdio JSON-RPC loop ──
//
// Concurrency: shared `ServerState` behind a Mutex, shared stdout behind another.
// LOCK ORDER INVARIANT — the two mutexes are NEVER held at the same time:
//   * request path: lock STATE → build response → DROP state lock → lock WRITER
//     → write → drop.
//   * watcher path: lock STATE → refresh_if_stale (stat + bounded plutil) →
//     compute `changed` → DROP state lock → if changed, lock WRITER → write
//     didChange → drop.
// No nesting → no lock-ordering cycle → no deadlock. The writer lock is never
// held across a subprocess. (The state lock may span a bounded `xcodebuild`
// during a query; that only delays the watcher's next poll — it cannot deadlock.)
// Mutexes are recovered from poisoning (`into_inner`), so a panic in one path
// never wedges the other; a dead watcher just stops proactive refresh.

fn run_stdio_loop() -> io::Result<()> {
    let state = Arc::new(Mutex::new(ServerState::empty()));
    let writer = Arc::new(Mutex::new(io::stdout()));
    let stop = Arc::new(AtomicBool::new(false));
    let mut watcher: Option<thread::JoinHandle<()>> = None;

    let stdin = io::stdin();
    let mut reader = stdin.lock();

    while let Some(body) = read_frame(&mut reader)? {
        // A malformed body is recoverable: the frame's bytes are consumed, so the
        // stream stays aligned. Reply with a parse error and keep serving.
        let msg: Value = match serde_json::from_slice(&body) {
            Ok(m) => m,
            Err(_) => {
                write_framed(&writer, &error_response(&Value::Null, -32700, "parse error"));
                continue;
            }
        };

        let method = msg.get("method").and_then(Value::as_str).unwrap_or("<none>").to_string();
        eprintln!("[xcode-bsp] received: {method}");

        // Build the response holding ONLY the state lock; release before writing.
        let response = {
            let mut st = lock(&state);
            handle_message(&mut st, &msg)
        };
        if let Some(response) = response {
            write_framed(&writer, &response);
        }

        // Start the watcher once the client is initialized (spec §6 / §2).
        if method == "build/initialized" && watcher.is_none() {
            watcher = Some(spawn_watcher(
                Arc::clone(&state),
                Arc::clone(&writer),
                Arc::clone(&stop),
            ));
        }

        // build/exit terminates the process (spec §7). Signal the watcher first,
        // then flush and exit.
        if method == "build/exit" {
            stop.store(true, Ordering::SeqCst);
            let mut w = lock(&writer);
            let _ = w.flush();
            std::process::exit(0);
        }
    }

    stop.store(true, Ordering::SeqCst);
    Ok(())
}

/// Lock a mutex, recovering the guard even if a previous holder panicked (so a
/// background panic can never wedge the server).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Write one framed message under the writer lock only (best-effort: a closed
/// stdout degrades silently rather than crashing).
fn write_framed(writer: &Arc<Mutex<io::Stdout>>, msg: &Value) {
    let mut w = lock(writer);
    let _ = write_message(&mut *w, msg);
}

/// `buildTarget/didChange` with `changes: null` = "all targets changed" →
/// SourceKit-LSP re-queries every open file (spec §6, the 2.2.0 path).
fn did_change_notification() -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "method": "buildTarget/didChange", "params": { "changes": null } })
}

// ── Background watcher (layer b): poll watched files, emit didChange on change ──

fn spawn_watcher(
    state: Arc<Mutex<ServerState>>,
    writer: Arc<Mutex<io::Stdout>>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(WATCH_POLL);
            if stop.load(Ordering::SeqCst) {
                break;
            }
            // STATE lock only: check + (maybe) invalidate + re-enumerate.
            let changed = {
                let mut st = lock(&state);
                st.refresh_if_stale()
            };
            // WRITER lock only, and only if something changed.
            if changed {
                write_framed(&writer, &did_change_notification());
            }
        }
    })
}

// ── Debug subcommand: run the real synth pipeline and print args (one/line) ──
// Usage: xcode-bsp synth (--project P | --workspace P) [--scheme S] --file F
// stdout = synthesized clang args (pipeable to `xcrun clang`); stderr = notes.

fn run_synth_command(args: &[String]) -> Result<(), String> {
    let mut project: Option<String> = None;
    let mut project_flag = "-project";
    let mut scheme: Option<String> = None;
    let mut file: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => {
                project = Some(next_value(args, &mut i)?);
                project_flag = "-project";
            }
            "--workspace" => {
                project = Some(next_value(args, &mut i)?);
                project_flag = "-workspace";
            }
            "--scheme" => scheme = Some(next_value(args, &mut i)?),
            "--file" => file = Some(next_value(args, &mut i)?),
            other => return Err(format!("unknown synth arg: {other}")),
        }
        i += 1;
    }

    let project = project.ok_or("synth: --project or --workspace is required")?;
    let file = file.ok_or("synth: --file is required")?;
    let file_path = PathBuf::from(&file);

    // Drive the real ServerState routing (identical path to the BSP handler).
    let mut st = ServerState::empty();
    st.project_flag = project_flag.to_string();
    st.project_path = project.clone();
    st.scheme = scheme;
    st.root = Path::new(&project).parent().map(Path::to_path_buf).unwrap_or_default();
    st.ensure_enumerated();

    let key = st
        .route(&file_path)
        .ok_or("synth: no target routes this file (no projects enumerated)")?;
    eprintln!("[xcode-bsp] routed {file} -> {}:{}", key.0, key.1);
    st.ensure_target(&key);

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let out_args: Vec<String> = if ext == "swift" {
        st.target_swift_args(&key)
            .map(<[String]>::to_vec)
            .ok_or("synth: no Swift args (no .swift sources or settings unavailable)")?
    } else {
        let settings = st
            .target_settings(&key)
            .ok_or("synth: settings unavailable for the routed target")?;
        synth::synthesize_clang_args(settings, &file_path, st.target_includes(&key))
    };

    for arg in out_args {
        println!("{arg}");
    }
    Ok(())
}

fn next_value(args: &[String], i: &mut usize) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("missing value for {}", args[*i - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trip_message() {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": Value::Null,
        });

        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();

        let mut reader = Cursor::new(buf);
        let body = read_frame(&mut reader).unwrap().unwrap();
        let decoded: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(decoded, msg);
        // Nothing left → clean EOF.
        assert!(read_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn read_frame_ignores_extra_headers() {
        // A Content-Type header (and header reordering) must not misframe.
        let raw = b"Content-Type: application/vscode-jsonrpc\r\nContent-Length: 2\r\n\r\n{}";
        let mut reader = Cursor::new(raw.to_vec());
        let body = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(body, b"{}");
    }

    #[test]
    fn content_length_is_utf8_byte_count() {
        // Path with Korean + emoji: char count != UTF-8 byte count.
        let msg = serde_json::json!({ "path": "/Users/me/프로젝트/📁/A.swift" });

        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();

        let text = String::from_utf8(buf).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        let declared: usize = header
            .strip_prefix("Content-Length:")
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        // Declared length must equal the body's byte length, and the multibyte
        // content must make byte length exceed char length.
        assert_eq!(declared, body.len());
        assert!(body.len() > body.chars().count());
    }

    #[test]
    fn uri_path_round_trips_with_spaces() {
        let p = Path::new("/Users/me/My Project/A.m");
        let uri = path_to_uri(p);
        assert!(uri.contains("%20"), "space must be percent-encoded: {uri}");
        assert_eq!(uri_to_path(&uri), "/Users/me/My Project/A.m");
    }

    // ── handle_message dispatch (no live xcodebuild needed) ──

    fn request(method: &str, params: Value) -> Value {
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
    }

    // An initialized state with no known project → enumeration/routing find no
    // target, so sourceKitOptions falls to the `result: null` path without
    // spawning xcodebuild.
    fn initialized_state() -> ServerState {
        let mut st = ServerState::empty();
        st.initialized = true;
        st
    }

    #[test]
    fn pre_init_request_errors_123() {
        let mut st = ServerState::empty();
        let resp = handle_message(&mut st, &request("workspace/buildTargets", Value::Null)).unwrap();
        assert_eq!(resp["error"]["code"], 123);
        assert!(resp.get("result").is_none());
    }

    #[test]
    fn initialize_advertises_sourcekit() {
        // Empty temp dir → no project detected → no live xcodebuild.
        let tmp = std::env::temp_dir().join(format!("xcode-bsp-init-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut st = ServerState::empty();
        let uri = format!("file://{}", tmp.to_string_lossy());
        let resp = handle_message(&mut st, &request("build/initialize", serde_json::json!({ "rootUri": uri }))).unwrap();
        std::fs::remove_dir_all(&tmp).ok();

        assert_eq!(resp["result"]["dataKind"], "sourceKit");
        assert_eq!(resp["result"]["data"]["sourceKitOptionsProvider"], true);
        assert_eq!(resp["result"]["bspVersion"], "2.2.0");
        assert!(st.initialized);
    }

    #[test]
    fn both_source_kit_options_names_route_to_same_handler() {
        let params = serde_json::json!({ "textDocument": { "uri": "file:///x/A.m" } });
        for method in ["textDocument/sourceKitOptions", "sourcekit/textDocument/sourceKitOptions"] {
            let mut st = initialized_state();
            let resp = handle_message(&mut st, &request(method, params.clone())).unwrap();
            // No project settings → null result (never an error / unhandled method).
            assert!(resp.get("result").is_some(), "{method} should reach the handler");
            assert!(resp["result"].is_null());
            assert!(resp.get("error").is_none());
        }
    }

    #[test]
    fn swift_uri_returns_null() {
        let mut st = initialized_state();
        let params = serde_json::json!({ "textDocument": { "uri": "file:///x/A.swift" } });
        let resp = handle_message(&mut st, &request("textDocument/sourceKitOptions", params)).unwrap();
        assert!(resp["result"].is_null());
    }

    #[test]
    fn unknown_method_errors_123() {
        let mut st = initialized_state();
        let resp = handle_message(&mut st, &request("frob/nonsense", Value::Null)).unwrap();
        assert_eq!(resp["error"]["code"], 123);
    }

    #[test]
    fn did_change_notification_has_null_changes_and_no_id() {
        let n = did_change_notification();
        assert_eq!(n["jsonrpc"], "2.0");
        assert_eq!(n["method"], "buildTarget/didChange");
        assert!(n["params"]["changes"].is_null());
        assert!(n.get("id").is_none()); // it's a notification
    }

    #[test]
    fn shutdown_returns_null() {
        let mut st = initialized_state();
        let resp = handle_message(&mut st, &request("build/shutdown", Value::Null)).unwrap();
        assert!(resp["result"].is_null());
        assert!(resp.get("error").is_none());
    }

    #[test]
    fn notifications_return_none() {
        let mut st = initialized_state();
        // build/initialized (no id) and $/cancelRequest (no id) → ignored.
        let notif = serde_json::json!({ "jsonrpc": "2.0", "method": "build/initialized", "params": {} });
        assert!(handle_message(&mut st, &notif).is_none());
        let cancel = serde_json::json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": {} });
        assert!(handle_message(&mut st, &cancel).is_none());
    }
}
