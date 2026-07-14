//! `plutil -convert json` invocation + pure PBXSourcesBuildPhase resolution.
//!
//! Ports the Phase 2 spike `parse_sources.py`: resolve a target's Sources phase
//! to absolute paths by walking the PBXGroup parent chain. See design §6.3.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::runner;

/// Bound on `plutil` (sub-second on a local pbxproj). Round B holds the state
/// lock across this, so a pathological stall (network mount / fifo) must not
/// freeze the request loop AND the watcher — time out and drop that project.
const PLUTIL_TIMEOUT_SECS: u64 = 30;

// ── Thin subprocess wrapper ──

pub fn run_plutil_json(pbxproj_path: &Path) -> Result<String, String> {
    let path = pbxproj_path.to_string_lossy();
    runner::run_bounded(
        "plutil",
        &["-convert", "json", "-o", "-", path.as_ref()],
        Duration::from_secs(PLUTIL_TIMEOUT_SECS),
    )
}

// ── Pure parsing ──

/// Resolve EVERY `PBXNativeTarget` to `(target_name, absolute source paths)`.
/// Used for multi-target workspaces. Target names are not globally unique across
/// projects, so callers must project-qualify.
pub fn parse_targets(
    json: &str,
    srcroot: &Path,
) -> Result<Vec<(String, Vec<PathBuf>)>, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("parse pbxproj json: {e}"))?;
    let objects = root
        .get("objects")
        .and_then(Value::as_object)
        .ok_or_else(|| "pbxproj: missing 'objects'".to_string())?;

    let mut parent_of: HashMap<&str, &str> = HashMap::new();
    for (oid, o) in objects {
        if matches!(isa(o), "PBXGroup" | "PBXVariantGroup") {
            if let Some(children) = o.get("children").and_then(Value::as_array) {
                for c in children.iter().filter_map(Value::as_str) {
                    parent_of.insert(c, oid.as_str());
                }
            }
        }
    }

    let mut targets: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for o in objects.values() {
        if isa(o) != "PBXNativeTarget" {
            continue;
        }
        let Some(name) = o.get("name").and_then(Value::as_str) else { continue };
        let mut sources: Vec<PathBuf> = Vec::new();
        if let Some(phases) = o.get("buildPhases").and_then(Value::as_array) {
            for phid in phases.iter().filter_map(Value::as_str) {
                let Some(phase) = objects.get(phid) else { continue };
                if isa(phase) != "PBXSourcesBuildPhase" {
                    continue;
                }
                let Some(files) = phase.get("files").and_then(Value::as_array) else { continue };
                for bfid in files.iter().filter_map(Value::as_str) {
                    let Some(bf) = objects.get(bfid) else { continue };
                    let Some(frid) = bf.get("fileRef").and_then(Value::as_str) else { continue };
                    if let Some(path) = resolve_fileref(objects, &parent_of, frid, srcroot) {
                        sources.push(path);
                    }
                }
            }
        }
        targets.push((name.to_string(), sources));
    }
    Ok(targets)
}

fn isa(o: &Value) -> &str {
    o.get("isa").and_then(Value::as_str).unwrap_or("")
}

fn resolve_fileref(
    objects: &Map<String, Value>,
    parent_of: &HashMap<&str, &str>,
    frid: &str,
    srcroot: &Path,
) -> Option<PathBuf> {
    let fr = objects.get(frid)?;
    let st = fr.get("sourceTree").and_then(Value::as_str).unwrap_or("<group>");
    let path = fr.get("path").and_then(Value::as_str).unwrap_or("");

    match st {
        "<absolute>" => Some(PathBuf::from(path)),
        "SOURCE_ROOT" => Some(lexical_normalize(&srcroot.join(path))),
        // Anchored outside the source tree — not an editable project source.
        "SDKROOT" | "DEVELOPER_DIR" | "BUILT_PRODUCTS_DIR" => None,
        _ => {
            // "<group>": accumulate paths up the group chain.
            let (anchor, prefix) = group_prefix(objects, parent_of, frid);
            let mut rel = PathBuf::new();
            for part in &prefix {
                rel.push(part);
            }
            if !path.is_empty() {
                rel.push(path);
            }
            match anchor.as_str() {
                "SOURCE_ROOT" | "<group>" => Some(lexical_normalize(&srcroot.join(rel))),
                _ => None,
            }
        }
    }
}

/// Accumulated group path from an anchor down to (not including) `oid`, walking
/// up the parent chain. Returns the anchor's `sourceTree` and the ordered parts.
fn group_prefix(
    objects: &Map<String, Value>,
    parent_of: &HashMap<&str, &str>,
    oid: &str,
) -> (String, Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut cur = parent_of.get(oid).copied();
    while let Some(cid) = cur {
        if !visited.insert(cid) {
            break; // cyclic group graph (broken/hand-edited pbxproj) — stop
        }
        let Some(g) = objects.get(cid) else { break };
        let st = g.get("sourceTree").and_then(Value::as_str).unwrap_or("<group>");
        if let Some(p) = g.get("path").and_then(Value::as_str) {
            if !p.is_empty() {
                parts.push(p.to_string());
            }
        }
        if matches!(
            st,
            "SOURCE_ROOT" | "<absolute>" | "SDKROOT" | "DEVELOPER_DIR" | "BUILT_PRODUCTS_DIR"
        ) {
            parts.reverse();
            return (st.to_string(), parts);
        }
        cur = parent_of.get(cid).copied();
    }
    parts.reverse();
    ("<group>".to_string(), parts)
}

/// Lexical path normalization (resolve `.`/`..` without touching the filesystem).
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal pbxproj-json: target "App" → Sources phase → two files in nested
    // groups (App/sub/main.m via a two-level chain; App/util.m one level).
    const FIXTURE: &str = r#"{
      "rootObject": "PROJ",
      "objects": {
        "PROJ": { "isa": "PBXProject", "mainGroup": "MG", "targets": ["TGT"] },
        "TGT": { "isa": "PBXNativeTarget", "name": "App", "buildPhases": ["SRC"] },
        "SRC": { "isa": "PBXSourcesBuildPhase", "files": ["BF1", "BF2"] },
        "BF1": { "isa": "PBXBuildFile", "fileRef": "FR1" },
        "BF2": { "isa": "PBXBuildFile", "fileRef": "FR2" },
        "FR1": { "isa": "PBXFileReference", "path": "main.m", "sourceTree": "<group>" },
        "FR2": { "isa": "PBXFileReference", "path": "util.m", "sourceTree": "<group>" },
        "MG": { "isa": "PBXGroup", "sourceTree": "<group>", "children": ["GROOT"] },
        "GROOT": { "isa": "PBXGroup", "path": "App", "sourceTree": "<group>", "children": ["GSUB", "FR2"] },
        "GSUB": { "isa": "PBXGroup", "path": "sub", "sourceTree": "<group>", "children": ["FR1"] }
      }
    }"#;

    // Sorted sources of the named target (test convenience over parse_targets).
    fn sources_of(json: &str, target: &str, srcroot: &Path) -> Vec<PathBuf> {
        let targets = parse_targets(json, srcroot).unwrap();
        let mut got = targets
            .into_iter()
            .find(|(name, _)| name == target)
            .map(|(_, s)| s)
            .unwrap_or_default();
        got.sort();
        got
    }

    #[test]
    fn resolves_nested_group_paths_via_parent_chain() {
        let got = sources_of(FIXTURE, "App", Path::new("/proj"));
        assert_eq!(
            got,
            vec![
                PathBuf::from("/proj/App/sub/main.m"),
                PathBuf::from("/proj/App/util.m"),
            ]
        );
    }

    #[test]
    fn parse_targets_returns_target_with_all_sources() {
        let targets = parse_targets(FIXTURE, Path::new("/proj")).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, "App");
        assert_eq!(targets[0].1.len(), 2);
    }

    #[test]
    fn cyclic_group_graph_terminates() {
        // GA and GB are each other's parent (broken pbxproj). Must return, not hang.
        const CYCLE: &str = r#"{
          "objects": {
            "TGT": { "isa": "PBXNativeTarget", "name": "App", "buildPhases": ["SRC"] },
            "SRC": { "isa": "PBXSourcesBuildPhase", "files": ["BF"] },
            "BF": { "isa": "PBXBuildFile", "fileRef": "FR" },
            "FR": { "isa": "PBXFileReference", "path": "a.m", "sourceTree": "<group>" },
            "GA": { "isa": "PBXGroup", "path": "A", "sourceTree": "<group>", "children": ["FR", "GB"] },
            "GB": { "isa": "PBXGroup", "path": "B", "sourceTree": "<group>", "children": ["GA"] }
          }
        }"#;
        let got = sources_of(CYCLE, "App", Path::new("/proj"));
        assert_eq!(got.len(), 1); // resolved (finite) without hanging
    }
}
