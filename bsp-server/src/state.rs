//! Server session state: multi-project/multi-target model with per-file routing.
//!
//! Populated on `build/initialize` from `<root>/buildServer.json` (custom
//! `project`/`project_flag`/`scheme` fields written by bsp-setup), falling back
//! to auto-detecting a single project. A `.xcworkspace` is enumerated into its
//! member `.xcodeproj`s; a lone `.xcodeproj` is a degenerate 1-project workspace
//! that flows through the SAME code path (keeps single-project projects working).
//!
//! Include scoping (Phase 3b spike): a workspace-wide `-I` scan collides sibling
//! headers ("duplicate interface"); each target's include set is scoped to its
//! own project dir + shared `common/`, excluding sibling projects and build dirs.
//! Index-store advertisement is best-effort for ONE (primary) project — a known
//! multi-index limitation.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::build_settings::{self, split_multi, BuildSettings};
use crate::pbxproj;
use crate::synth;

/// Names are NOT globally unique across projects — always project-qualify.
pub type TargetKey = (String /* project .xcodeproj path */, String /* target name */);

pub struct ServerState {
    pub initialized: bool,
    pub root: PathBuf,
    pub project_flag: String,
    pub project_path: String,
    pub scheme: Option<String>,

    // Enumeration (eager, cheap: plutil + pbxproj parse).
    enumerated: bool,
    workspace_root: PathBuf,
    projects: Vec<PathBuf>,
    project_dirs: Vec<PathBuf>,
    // Non-project workspace children (e.g. `common/`) shared by all targets.
    shared_dirs: Vec<PathBuf>,
    file_to_targets: HashMap<PathBuf, Vec<TargetKey>>,
    target_sources: HashMap<TargetKey, Vec<PathBuf>>,
    ordered_targets: Vec<TargetKey>,

    // Watched files + last-seen mtimes for cache invalidation (project.pbxproj,
    // shallow *.xcconfig, Package.resolved). Refreshed on each enumeration.
    watched: Vec<(PathBuf, Option<SystemTime>)>,

    // Lazy per-target caches (populated on first query for that target).
    schemes: Option<Vec<String>>,
    target_settings: HashMap<TargetKey, Option<BuildSettings>>,
    target_includes: HashMap<TargetKey, Vec<String>>,
    target_swift_args: HashMap<TargetKey, Option<Vec<String>>>,
}

impl ServerState {
    pub fn empty() -> Self {
        ServerState {
            initialized: false,
            root: PathBuf::new(),
            project_flag: String::new(),
            project_path: String::new(),
            scheme: None,
            enumerated: false,
            workspace_root: PathBuf::new(),
            projects: Vec::new(),
            project_dirs: Vec::new(),
            shared_dirs: Vec::new(),
            file_to_targets: HashMap::new(),
            target_sources: HashMap::new(),
            ordered_targets: Vec::new(),
            watched: Vec::new(),
            schemes: None,
            target_settings: HashMap::new(),
            target_includes: HashMap::new(),
            target_swift_args: HashMap::new(),
        }
    }

    /// Resolve project + scheme from `<root>/buildServer.json` (preferred) or by
    /// auto-detecting a single project under `root`.
    pub fn init_from_root(root: &Path) -> Self {
        let mut st = ServerState::empty();
        st.root = root.to_path_buf();
        if let Some((flag, path, scheme)) = read_build_server_json(root) {
            st.project_flag = flag;
            st.project_path = path;
            st.scheme = scheme;
        } else if let Some((flag, path)) = detect_project(root) {
            st.project_flag = flag;
            st.project_path = path;
        }
        st
    }

    // ── Enumeration ──

    /// Enumerate projects/targets and build the file→target multimap. Idempotent.
    pub fn ensure_enumerated(&mut self) {
        if self.enumerated {
            return;
        }
        if self.project_path.is_empty() {
            // Degenerate (no project resolved): leave `enumerated == false` so a
            // later call retries cheaply. Nothing to build.
            return;
        }
        let proj = Path::new(&self.project_path);
        self.workspace_root = proj.parent().map(Path::to_path_buf).unwrap_or_default();

        let projects: Vec<PathBuf> = if self.project_flag == "-workspace" {
            let (projects, shared) = read_workspace_members(proj);
            self.shared_dirs = shared;
            projects
        } else {
            vec![proj.to_path_buf()]
        };
        // Record the resolved project list up front so the watched-file set
        // includes their pbxproj even if a (mid-write) parse fails below —
        // otherwise a failed re-enumeration would stop watching and never notice
        // the file settling.
        self.projects = projects.clone();

        // Pre-read pbxproj mtimes (Fix 1): sample BEFORE reading content, so a
        // save that completes during the read→stat gap is still noticed next tick
        // (biases to one extra re-enumeration, never a missed change).
        let mut pbx_watched: Vec<(PathBuf, Option<SystemTime>)> = Vec::new();

        for xcodeproj in &projects {
            let project_dir = xcodeproj.parent().map(Path::to_path_buf).unwrap_or_default();
            self.project_dirs.push(project_dir.clone());
            let pbx = xcodeproj.join("project.pbxproj");
            let pbx_mtime = mtime_of(&pbx); // sample BEFORE run_plutil_json reads it
            pbx_watched.push((pbx.clone(), pbx_mtime));

            // Diagnose a silently-dropped project (e.g. a nested-Group location we
            // couldn't resolve, or a moved project) rather than failing quietly.
            if !xcodeproj.exists() {
                eprintln!("[xcode-bsp] warning: workspace references missing project: {}", xcodeproj.display());
                continue;
            }
            let Ok(json) = pbxproj::run_plutil_json(&pbx) else {
                continue;
            };
            let Ok(targets) = pbxproj::parse_targets(&json, &project_dir) else {
                continue;
            };
            let proj_str = xcodeproj.to_string_lossy().into_owned();
            for (tname, sources) in targets {
                let key: TargetKey = (proj_str.clone(), tname);
                // Existence-filter: skip stale pbxproj entries (must not error).
                let existing: Vec<PathBuf> = sources.into_iter().filter(|p| p.is_file()).collect();
                for f in &existing {
                    self.file_to_targets.entry(f.clone()).or_default().push(key.clone());
                }
                self.ordered_targets.push(key.clone());
                self.target_sources.insert(key, existing);
            }
        }
        self.ordered_targets.sort();
        self.ordered_targets.dedup();
        for v in self.file_to_targets.values_mut() {
            v.sort();
            v.dedup();
        }
        self.watched = self.collect_watched(pbx_watched);
        // Fix 2: flip the flag LAST — a panic above leaves it `false`, so a
        // poison-recovered lock re-enumerates cleanly instead of serving a
        // half-built map forever.
        self.enumerated = true;
    }

    // ── Cache invalidation (Round B) ──

    /// Cheap query-time staleness check (stat only). If any watched file's mtime
    /// changed / it appeared / disappeared since the last enumeration, invalidate
    /// all caches and re-enumerate, then return true. No-op before first enum.
    pub fn refresh_if_stale(&mut self) -> bool {
        if !self.enumerated || !self.watched_changed() {
            return false;
        }
        self.invalidate();
        self.ensure_enumerated();
        true
    }

    fn watched_changed(&self) -> bool {
        self.watched.iter().any(|(p, prev)| mtime_of(p) != *prev)
    }

    /// Reset all enumeration/cache state (keeps the identity: root/project/scheme).
    fn invalidate(&mut self) {
        self.enumerated = false;
        self.workspace_root = PathBuf::new();
        self.projects.clear();
        self.project_dirs.clear();
        self.shared_dirs.clear();
        self.file_to_targets.clear();
        self.target_sources.clear();
        self.ordered_targets.clear();
        self.watched.clear();
        self.schemes = None;
        self.target_settings.clear();
        self.target_includes.clear();
        self.target_swift_args.clear();
    }

    /// The set of files whose change invalidates caches, with mtimes. The
    /// pbxproj entries are pre-sampled (mtime taken BEFORE the content read — see
    /// Fix 1); the stat-only candidates (shallow `*.xcconfig` in project dirs,
    /// `Package.resolved` at workspace root + project dirs) are stat'd here.
    /// Absent candidates are stored with `None` so their appearance is detected.
    fn collect_watched(
        &self,
        pbx_watched: Vec<(PathBuf, Option<SystemTime>)>,
    ) -> Vec<(PathBuf, Option<SystemTime>)> {
        let mut watched = pbx_watched;
        let mut stat_only: Vec<PathBuf> = Vec::new();
        for dir in &self.project_dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("xcconfig") {
                        stat_only.push(p);
                    }
                }
            }
        }
        stat_only.push(self.workspace_root.join("Package.resolved"));
        for dir in &self.project_dirs {
            stat_only.push(dir.join("Package.resolved"));
        }
        for p in stat_only {
            let m = mtime_of(&p);
            watched.push((p, m));
        }
        watched.sort_by(|a, b| a.0.cmp(&b.0));
        watched.dedup_by(|a, b| a.0 == b.0); // keeps the first (pre-sampled pbxproj wins)
        watched
    }

    // ── Routing ──

    /// Route a file to its owning target: exact source membership (deterministic
    /// primary if shared), else a project-dir prefix match, else the global
    /// primary. `None` only when no targets were enumerated.
    pub fn route(&self, file: &Path) -> Option<TargetKey> {
        if let Some(targets) = self.file_to_targets.get(file) {
            if let Some(first) = targets.first() {
                return Some(first.clone()); // sorted → deterministic primary
            }
        }
        // Not in any pbxproj (e.g. a new file): prefer a target whose project dir
        // contains the file.
        let mut prefix: Vec<&TargetKey> = self
            .ordered_targets
            .iter()
            .filter(|(proj, _)| {
                Path::new(proj)
                    .parent()
                    .map(|pd| file.starts_with(pd))
                    .unwrap_or(false)
            })
            .collect();
        prefix.sort();
        if let Some(k) = prefix.first() {
            return Some((*k).clone());
        }
        self.primary_target()
    }

    /// Deterministic global primary target (alphabetically-first).
    pub fn primary_target(&self) -> Option<TargetKey> {
        self.ordered_targets.first().cloned()
    }

    // ── Per-target lazy load ──

    /// Load a target's settings, scoped include dirs, and (if any `.swift`
    /// sources) whole-module Swift args. Idempotent per target.
    pub fn ensure_target(&mut self, key: &TargetKey) {
        if self.target_includes.contains_key(key) {
            return;
        }
        self.load_target_settings(key);

        // HSP as owned data so the settings borrow is dropped before scanning.
        let hsp_set: HashSet<String> = match self.target_settings.get(key).and_then(|o| o.as_ref())
        {
            Some(s) => split_multi(s.get("HEADER_SEARCH_PATHS").unwrap_or("")).into_iter().collect(),
            None => {
                // No settings → nothing usable; mark processed and degrade.
                self.target_includes.insert(key.clone(), Vec::new());
                self.target_swift_args.insert(key.clone(), None);
                return;
            }
        };

        let roots = self.scan_roots_for(key);
        let includes = synth::scoped_include_dirs(&roots, &hsp_set);
        self.target_includes.insert(key.clone(), includes);

        // Whole-module Swift args (identical for every .swift in the target).
        let swift_files: Vec<PathBuf> = self
            .target_sources
            .get(key)
            .map(|v| {
                v.iter()
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("swift"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let swift_args: Option<Vec<String>> = if swift_files.is_empty() {
            None
        } else if let Some(cache) = self.module_cache_dir() {
            let settings = self.target_settings.get(key).and_then(|o| o.as_ref());
            let includes = self.target_includes.get(key).map(Vec::as_slice).unwrap_or(&[]);
            settings.map(|s| {
                synth::synthesize_swift_args(s, &swift_files, &self.workspace_root, includes, &cache)
            })
        } else {
            None
        };
        self.target_swift_args.insert(key.clone(), swift_args);
    }

    fn load_target_settings(&mut self, key: &TargetKey) {
        if self.target_settings.contains_key(key) {
            return;
        }
        let (proj_path, tname) = key;
        let project_dir = Path::new(proj_path).parent().map(Path::to_path_buf).unwrap_or_default();
        self.ensure_schemes();
        let has_scheme = self
            .schemes
            .as_ref()
            .map(|s| s.iter().any(|n| n == tname))
            .unwrap_or(false);

        let settings: Option<BuildSettings> = if self.project_flag == "-project" {
            // Single project. Honor an explicit `-s` from buildServer.json (Fix 2:
            // the multi-scheme fail-safe forces the flag, so it must take effect);
            // else an eponymous scheme; else the target directly.
            let args = if let Some(s) = self.scheme.as_deref() {
                sbs_args(&["-project", proj_path, "-scheme", s])
            } else if has_scheme {
                sbs_args(&["-project", proj_path, "-scheme", tname])
            } else {
                sbs_args(&["-project", proj_path, "-target", tname])
            };
            load_settings(&args, tname)
        } else {
            // Workspace. Prefer `-workspace -scheme <tname>` (keeps the shared
            // DerivedData / index store), but ONLY if the resulting settings
            // belong to THIS target's project — guards against a same-named
            // scheme owned by another project (Fix 1). Never a bare-name match.
            let via_scheme = if has_scheme {
                load_settings(
                    &sbs_args(&[
                        self.project_flag.as_str(),
                        self.project_path.as_str(),
                        "-scheme",
                        tname,
                    ]),
                    tname,
                )
                .filter(|s| settings_belong_to(s, &project_dir))
            } else {
                None
            };
            // Fallback: project-qualified (correct settings; may forgo the
            // workspace index for the rare same-name case).
            via_scheme.or_else(|| {
                load_settings(&sbs_args(&["-project", proj_path, "-target", tname]), tname)
            })
        };

        self.target_settings.insert(key.clone(), settings);
    }

    fn ensure_schemes(&mut self) {
        if self.schemes.is_some() {
            return;
        }
        let args = vec![
            "-list".to_string(),
            "-json".to_string(),
            self.project_flag.clone(),
            self.project_path.clone(),
        ];
        let schemes = build_settings::run_xcodebuild(&args)
            .map(|json| build_settings::parse_scheme_list(&json))
            .unwrap_or_default();
        self.schemes = Some(schemes);
    }

    /// Scan roots for a target's scoped include set: its own project dir plus the
    /// shared non-project workspace dirs (`common/`). Sibling project dirs are
    /// intentionally excluded (a workspace-wide scan collides sibling headers).
    fn scan_roots_for(&self, key: &TargetKey) -> Vec<PathBuf> {
        let (proj_path, _) = key;
        let project_dir = Path::new(proj_path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut roots = vec![project_dir];
        for d in &self.shared_dirs {
            // Never scan a project dir, its subtree, or an ANCESTOR of one (an
            // ancestor/workspace-root shared dir would pull in sibling headers
            // and re-introduce the cross-project collision).
            let touches_project = self
                .project_dirs
                .iter()
                .any(|pd| d == pd || d.starts_with(pd) || pd.starts_with(d.as_path()));
            if !touches_project && !roots.contains(d) {
                roots.push(d.clone());
            }
        }
        roots
    }

    // ── Index advertisement (best-effort, one primary project) ──

    /// The single index store to advertise. BSP `indexStorePath` is SINGULAR (a
    /// SourceKit-LSP protocol constraint) — separate per-project stores can't all
    /// be advertised, so we target the workspace-shared store. Preferred: the
    /// primary target's derived store (the shared store when built via
    /// `-workspace`); fallback: the largest existing store among all targets.
    /// This is a documented limit, not a bug.
    pub fn primary_index_store(&mut self) -> Option<PathBuf> {
        self.ensure_enumerated();
        if let Some(primary) = self.primary_target() {
            self.ensure_target(&primary);
            if let Some(idx) = self
                .target_settings
                .get(&primary)
                .and_then(|o| o.as_ref())
                .and_then(build_settings::derive_index_store_path)
            {
                return Some(idx); // derive_index_store_path already existence-gates
            }
        }
        self.largest_existing_index_store()
    }

    /// Fallback (primary store absent): scan the DEFAULT DerivedData directory by
    /// name — `<name>-<hash>/Index.noindex/DataStore` for `<name>` in the
    /// workspace basename and each project basename — and pick the largest.
    /// NO `xcodebuild`/settings loads here (keeps `build/initialize` fast on a
    /// never-built multi-project workspace).
    ///
    /// Assumes the default DerivedData location. A custom `-derivedDataPath` with
    /// a never-built primary yields no index (rare, acceptable); the primary
    /// settings-derived path already covers custom locations once a build exists.
    fn largest_existing_index_store(&self) -> Option<PathBuf> {
        let mut names: Vec<String> = Vec::new();
        let mut add_stem = |p: &Path| {
            if let Some(s) = p.file_stem().and_then(|s| s.to_str()) {
                let s = s.to_string();
                if !names.contains(&s) {
                    names.push(s);
                }
            }
        };
        add_stem(Path::new(&self.project_path)); // workspace (or lone project) file stem
        for (proj_path, _) in &self.ordered_targets {
            add_stem(Path::new(proj_path)); // each project's .xcodeproj file stem
        }

        let home = std::env::var_os("HOME")?;
        let dd = Path::new(&home).join("Library/Developer/Xcode/DerivedData");
        largest_index_store_in(&dd, &names)
    }

    // ── Getters ──

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn target_settings(&self, key: &TargetKey) -> Option<&BuildSettings> {
        self.target_settings.get(key).and_then(|o| o.as_ref())
    }

    pub fn target_includes(&self, key: &TargetKey) -> &[String] {
        self.target_includes.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn target_swift_args(&self, key: &TargetKey) -> Option<&[String]> {
        self.target_swift_args.get(key).and_then(|o| o.as_deref())
    }

    /// Writable, project-unique cache dir for SourceKit-LSP's own index DB.
    pub fn index_database_path(&self) -> Option<PathBuf> {
        let dir = self.cache_subdir("Library/Caches/xcode-tools-bsp")?;
        Some(dir.join("db"))
    }

    /// Tree-external module/PCH cache dir for swiftc, project-unique. Created.
    pub fn module_cache_dir(&self) -> Option<PathBuf> {
        self.cache_subdir(".config/zed/xcode-tools/modcache")
    }

    fn cache_subdir(&self, under: &str) -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        let sanitized: String = self
            .root
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let dir = Path::new(&home).join(under).join(sanitized);
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }
}

// ── Build-settings loading helpers (no `self` borrow, keeps the borrow checker happy) ──

fn sbs_args(tail: &[&str]) -> Vec<String> {
    let mut v = vec!["-showBuildSettings".to_string(), "-json".to_string()];
    v.extend(tail.iter().map(|s| s.to_string()));
    v
}

fn load_settings(args: &[String], target: &str) -> Option<BuildSettings> {
    build_settings::run_xcodebuild(args)
        .ok()
        .and_then(|json| build_settings::parse_build_settings_for_target(&json, Some(target)).ok())
}

/// Pure: among `<name>-<hash>` entries under a DerivedData dir (for the given
/// names), the largest existing `Index.noindex/DataStore`. No xcodebuild.
fn largest_index_store_in(deriveddata: &Path, names: &[String]) -> Option<PathBuf> {
    let entries = std::fs::read_dir(deriveddata).ok()?;
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let matches = names
            .iter()
            .any(|n| name.strip_prefix(n.as_str()).is_some_and(|rest| rest.starts_with('-')));
        if !matches {
            continue;
        }
        let store = entry.path().join("Index.noindex").join("DataStore");
        if store.is_dir() {
            let size = dir_size(&store);
            if best.as_ref().map(|(b, _)| size > *b).unwrap_or(true) {
                best = Some((size, store));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Modification time of a file, or `None` if it doesn't exist / can't be stat'd
/// (a transient read failure mid-save reads as "no change yet").
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Total byte size of all files under `path` (recursive). Used to pick the
/// largest index store in the fallback; symlinked dirs are not followed.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => {
                    if let Ok(m) = e.metadata() {
                        total += m.len();
                    }
                }
                _ => {}
            }
        }
    }
    total
}

/// True when these settings describe the given project (guards against a
/// same-named scheme owned by a different project in a workspace).
fn settings_belong_to(settings: &BuildSettings, project_dir: &Path) -> bool {
    settings
        .get("PROJECT_DIR")
        .map(|p| Path::new(p) == project_dir)
        .unwrap_or(false)
}

// ── Workspace enumeration (contents.xcworkspacedata) ──

fn read_workspace_members(ws: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(ws_root) = ws.parent() else { return (Vec::new(), Vec::new()) };
    let Ok(xml) = std::fs::read_to_string(ws.join("contents.xcworkspacedata")) else {
        return (Vec::new(), Vec::new());
    };
    resolve_workspace_members(ws_root, &xml)
}

/// Pure: classify `contents.xcworkspacedata` FileRef `location`s into referenced
/// `.xcodeproj` paths and shared non-project dirs (e.g. `common/`).
///
/// Nesting-aware: a dependency-free stack-based tag scanner accumulates enclosing
/// `<Group location=...>` prefixes so projects nested in groups resolve to the
/// correct path. Location schemes: `group:<rel>` is relative to the enclosing
/// group (or the ws dir at top level); `container:<rel>` and `self:[<rel>]` are
/// relative to the workspace document's dir; `absolute:<abs>` is absolute.
fn resolve_workspace_members(ws_root: &Path, xml: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut projects = Vec::new();
    let mut shared = Vec::new();
    let mut group_stack: Vec<PathBuf> = Vec::new();

    for tag in iter_tags(xml) {
        let body = tag.trim_start();
        if body.starts_with("/Group") {
            group_stack.pop();
            continue;
        }
        // Self-closing tag content ends with `/` (attribute values are quoted,
        // so a trailing `/` inside a value can't be confused for `/>`).
        let self_closing = tag.trim_end().ends_with('/');
        let base = group_stack.last().map(PathBuf::as_path).unwrap_or(ws_root);

        if body.starts_with("Group") {
            let resolved = tag_location(tag)
                .map(|loc| resolve_location(&loc, ws_root, base))
                .unwrap_or_else(|| base.to_path_buf());
            if !self_closing {
                group_stack.push(resolved); // opens a scope closed by </Group>
            }
        } else if body.starts_with("FileRef") {
            if let Some(loc) = tag_location(tag) {
                let resolved = resolve_location(&loc, ws_root, base);
                if resolved.extension().and_then(|e| e.to_str()) == Some("xcodeproj") {
                    projects.push(resolved);
                } else {
                    shared.push(resolved);
                }
            }
        }
    }
    (projects, shared)
}

/// Resolve one workspace `location` against the ws dir and the enclosing group.
fn resolve_location(loc: &str, ws_root: &Path, group_base: &Path) -> PathBuf {
    let (prefix, rest) = loc.split_once(':').unwrap_or(("group", loc));
    match prefix {
        "absolute" => lexical_normalize(Path::new(rest)),
        "container" | "self" => lexical_normalize(&ws_root.join(rest)),
        _ => lexical_normalize(&group_base.join(rest)), // "group" + default
    }
}

/// The interiors (between `<` and `>`) of every tag in the XML.
fn iter_tags(xml: &str) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        let after = &rest[lt + 1..];
        match after.find('>') {
            Some(gt) => {
                tags.push(&after[..gt]);
                rest = &after[gt + 1..];
            }
            None => break,
        }
    }
    tags
}

/// The `location` attribute value of a single tag, if any.
fn tag_location(tag: &str) -> Option<String> {
    extract_locations(tag).into_iter().next()
}

/// Extract the value of each `location = "..."` attribute (avoids an XML-parser
/// dependency). Anchors on the `=` after `location` and consumes the whole
/// quoted value, so a `location` substring inside a path is neither a false
/// match nor a truncation.
fn extract_locations(xml: &str) -> Vec<String> {
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = xml[i..].find("location") {
        let mut j = i + rel + "location".len();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'=' {
            // Opening quote, then the value up to the closing quote.
            if let Some(qrel) = xml[j..].find('"') {
                let start = j + qrel + 1;
                if let Some(erel) = xml[start..].find('"') {
                    out.push(xml[start..start + erel].to_string());
                    i = start + erel + 1;
                    continue;
                }
            }
        }
        i += rel + "location".len();
    }
    out
}

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

// ── buildServer.json + single-project auto-detect ──

fn read_build_server_json(root: &Path) -> Option<(String, String, Option<String>)> {
    let content = std::fs::read_to_string(root.join("buildServer.json")).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    let project = v
        .get("project")
        .and_then(Value::as_str)
        .or_else(|| v.get("workspace").and_then(Value::as_str))?;
    let flag = v
        .get("project_flag")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if project.ends_with(".xcworkspace") {
                "-workspace".to_string()
            } else {
                "-project".to_string()
            }
        });
    let scheme = v.get("scheme").and_then(Value::as_str).map(str::to_string);
    Some((flag, project.to_string(), scheme))
}

fn detect_project(root: &Path) -> Option<(String, String)> {
    let mut workspaces = Vec::new();
    let mut projects = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            match p.extension().and_then(|x| x.to_str()) {
                Some("xcworkspace") => workspaces.push(p),
                Some("xcodeproj") => projects.push(p),
                _ => {}
            }
        }
    }
    if workspaces.len() == 1 {
        return Some(("-workspace".to_string(), workspaces[0].to_string_lossy().into_owned()));
    }
    if projects.len() == 1 {
        return Some(("-project".to_string(), projects[0].to_string_lossy().into_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const WS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Workspace version = "1.0">
   <FileRef location = "group:common"></FileRef>
   <FileRef location = "group:RVmacRXViewerManager/RVmacRXViewerManager.xcodeproj"></FileRef>
   <FileRef location = "group:RVmacRXViewerDisplay/RVmacRXViewerDisplay.xcodeproj"></FileRef>
   <FileRef location = "group:RVmacRXViewerStarter/RVmacRXViewerStarter.xcodeproj"></FileRef>
</Workspace>"#;

    #[test]
    fn workspace_members_classify_projects_and_shared_dirs() {
        let ws = Path::new("/ws");
        let (projs, shared) = resolve_workspace_members(ws, WS_XML);
        assert_eq!(
            projs,
            vec![
                PathBuf::from("/ws/RVmacRXViewerManager/RVmacRXViewerManager.xcodeproj"),
                PathBuf::from("/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay.xcodeproj"),
                PathBuf::from("/ws/RVmacRXViewerStarter/RVmacRXViewerStarter.xcodeproj"),
            ]
        );
        // group:common (a dir, not .xcodeproj) → shared, not a project.
        assert_eq!(shared, vec![PathBuf::from("/ws/common")]);
    }

    #[test]
    fn extract_locations_handles_location_substring_in_path() {
        // A path segment literally containing "location" must not truncate/drop.
        let xml = r#"<FileRef location = "group:my_location/App.xcodeproj"></FileRef>"#;
        assert_eq!(extract_locations(xml), vec!["group:my_location/App.xcodeproj".to_string()]);
    }

    #[test]
    fn nested_group_resolves_project_to_nested_path() {
        // <Group group:sub> → <Group group:deep> → <FileRef group:App.xcodeproj>
        let xml = r#"<?xml version="1.0"?><Workspace version="1.0">
          <Group location = "group:sub">
            <Group location = "group:deep">
              <FileRef location = "group:App.xcodeproj"></FileRef>
            </Group>
          </Group>
        </Workspace>"#;
        let (projs, _) = resolve_workspace_members(Path::new("/ws"), xml);
        assert_eq!(projs, vec![PathBuf::from("/ws/sub/deep/App.xcodeproj")]);
    }

    #[test]
    fn container_scheme_ignores_group_nesting() {
        // `container:` resolves against the ws dir regardless of enclosing groups;
        // `group:` accumulates the group prefix.
        let xml = r#"<Workspace>
          <Group location = "group:sub">
            <FileRef location = "container:Top/C.xcodeproj"></FileRef>
            <FileRef location = "group:G.xcodeproj"></FileRef>
          </Group>
        </Workspace>"#;
        let (projs, _) = resolve_workspace_members(Path::new("/ws"), xml);
        assert!(projs.contains(&PathBuf::from("/ws/Top/C.xcodeproj")));
        assert!(projs.contains(&PathBuf::from("/ws/sub/G.xcodeproj")));
    }

    #[test]
    fn self_closing_group_does_not_nest_siblings() {
        // A self-closing <Group/> has no children; the sibling FileRef stays top-level.
        let xml = r#"<Workspace>
          <Group location = "group:sub" />
          <FileRef location = "group:App.xcodeproj" />
        </Workspace>"#;
        let (projs, _) = resolve_workspace_members(Path::new("/ws"), xml);
        assert_eq!(projs, vec![PathBuf::from("/ws/App.xcodeproj")]);
    }

    #[test]
    fn flat_workspace_still_resolves_identically() {
        // Regression: the real flat layout parses exactly as before.
        let (projs, shared) = resolve_workspace_members(Path::new("/ws"), WS_XML);
        assert_eq!(projs.len(), 3);
        assert_eq!(shared, vec![PathBuf::from("/ws/common")]);
    }

    #[test]
    fn index_fallback_scans_deriveddata_by_name_and_picks_largest() {
        // Fake DerivedData with two matching stores + an unrelated (huge) one.
        // No xcodebuild — pure filesystem name scan.
        let tmp = std::env::temp_dir().join(format!("xcode-bsp-dd-{}", std::process::id()));
        let mk = |name: &str, bytes: usize| -> PathBuf {
            let store = tmp.join(name).join("Index.noindex").join("DataStore");
            std::fs::create_dir_all(&store).unwrap();
            std::fs::write(store.join("data"), vec![0u8; bytes]).unwrap();
            store
        };
        let _small = mk("MyApp-aaaa", 16);
        let large = mk("MyApp-bbbb", 4096);
        let _unrelated = mk("OtherProj-cccc", 1_000_000); // name not requested → ignored

        let names = vec!["MyApp".to_string()];
        let got = largest_index_store_in(&tmp, &names);

        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(got, Some(large)); // larger MyApp store wins; unrelated ignored
    }

    #[test]
    fn watched_changed_detects_mtime_appearance_disappearance() {
        let tmp = std::env::temp_dir().join(format!("xcode-bsp-watch-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let f = tmp.join("project.pbxproj");
        std::fs::write(&f, b"x").unwrap();
        let real = mtime_of(&f).unwrap();

        let mut st = ServerState::empty();
        // Stored == actual → unchanged.
        st.watched = vec![(f.clone(), Some(real))];
        assert!(!st.watched_changed());
        // Stored older than actual → changed.
        st.watched = vec![(f.clone(), real.checked_sub(std::time::Duration::from_secs(100)))];
        assert!(st.watched_changed());
        // Appearance: absent candidate that shows up → changed.
        let g = tmp.join("Package.resolved");
        st.watched = vec![(g.clone(), None)];
        assert!(!st.watched_changed());
        std::fs::write(&g, b"y").unwrap();
        assert!(st.watched_changed());
        // Disappearance: present file removed → changed.
        st.watched = vec![(f.clone(), Some(real))];
        std::fs::remove_file(&f).unwrap();
        assert!(st.watched_changed());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn refresh_if_stale_is_noop_before_enumeration() {
        let mut st = ServerState::empty();
        st.enumerated = false;
        // A "changed" entry must not trigger a refresh before the first enumeration.
        st.watched = vec![(PathBuf::from("/nope/project.pbxproj"), Some(SystemTime::now()))];
        assert!(!st.refresh_if_stale());
    }

    #[test]
    fn collect_watched_keeps_presampled_pbxproj_mtime() {
        // Fix 1: the pbxproj entry must retain the PRE-READ sample, not a re-stat.
        let tmp = std::env::temp_dir().join(format!("xcode-bsp-cw-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let pbx = tmp.join("project.pbxproj");
        std::fs::write(&pbx, b"x").unwrap();
        let stale = mtime_of(&pbx).unwrap().checked_sub(std::time::Duration::from_secs(500));

        let mut st = ServerState::empty();
        st.workspace_root = tmp.clone();
        let w = st.collect_watched(vec![(pbx.clone(), stale)]);

        let got = w.iter().find(|(p, _)| p == &pbx).map(|(_, m)| *m);
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(got, Some(stale), "pre-sampled pbxproj mtime must be preserved");
    }

    #[test]
    fn empty_project_path_stays_unenumerated_for_retry() {
        // Fix 2: no project resolved → `enumerated` stays false so a later call
        // retries (and a panic mid-build likewise leaves it false → self-heal).
        let mut st = ServerState::empty();
        assert!(st.project_path.is_empty());
        st.ensure_enumerated();
        assert!(!st.enumerated);
    }

    #[test]
    fn settings_belong_to_matches_project_dir() {
        let json = r#"[{"target":"T","buildSettings":{"PROJECT_DIR":"/ws/ProjA"}}]"#;
        let s = build_settings::parse_build_settings_for_target(json, None).unwrap();
        assert!(settings_belong_to(&s, Path::new("/ws/ProjA")));
        assert!(!settings_belong_to(&s, Path::new("/ws/ProjB")));
    }

    #[test]
    fn scan_roots_excludes_workspace_root_ancestor_shared_dir() {
        // A shared dir that is an ANCESTOR of the projects (e.g. the ws root
        // itself) must be excluded — else it re-introduces the sibling collision.
        let d = "/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay.xcodeproj";
        let mut st = state_with(
            "/ws",
            &["/ws/RVmacRXViewerDisplay", "/ws/RVmacRXViewerManager"],
            &["/ws"], // pathological shared dir == workspace root (ancestor of projects)
            &[],
            &[((d, "Display"), &[])],
        );
        st.shared_dirs = vec![PathBuf::from("/ws")];
        let roots = st.scan_roots_for(&(d.to_string(), "Display".to_string()));
        assert_eq!(roots, vec![PathBuf::from("/ws/RVmacRXViewerDisplay")]);
    }

    // Build a ServerState with an injected enumeration for routing/scope tests.
    fn state_with(
        workspace_root: &str,
        project_dirs: &[&str],
        shared_dirs: &[&str],
        file_to_targets: &[(&str, &[(&str, &str)])],
        target_sources: &[((&str, &str), &[&str])],
    ) -> ServerState {
        let mut st = ServerState::empty();
        st.enumerated = true;
        st.workspace_root = PathBuf::from(workspace_root);
        st.project_dirs = project_dirs.iter().map(PathBuf::from).collect();
        st.shared_dirs = shared_dirs.iter().map(PathBuf::from).collect();
        for (f, tks) in file_to_targets {
            let v: Vec<TargetKey> =
                tks.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
            st.file_to_targets.insert(PathBuf::from(*f), v);
        }
        for ((p, t), srcs) in target_sources {
            let key = (p.to_string(), t.to_string());
            st.ordered_targets.push(key.clone());
            st.target_sources
                .insert(key, srcs.iter().map(PathBuf::from).collect());
        }
        st.ordered_targets.sort();
        st.ordered_targets.dedup();
        for v in st.file_to_targets.values_mut() {
            v.sort();
        }
        st
    }

    fn rvmac_state() -> ServerState {
        let d = "/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay.xcodeproj";
        let m = "/ws/RVmacRXViewerManager/RVmacRXViewerManager.xcodeproj";
        state_with(
            "/ws",
            &[
                "/ws/RVmacRXViewerDisplay",
                "/ws/RVmacRXViewerManager",
                "/ws/RVmacRXViewerStarter",
            ],
            &["/ws/common"],
            &[
                // a common/ file compiled by BOTH Display and Manager
                ("/ws/common/shared.m", &[(d, "Display"), (m, "Manager")]),
                // a Display-only project file
                ("/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay/main.m", &[(d, "Display")]),
            ],
            &[
                (
                    (d, "Display"),
                    &["/ws/common/shared.m", "/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay/main.m"],
                ),
                ((m, "Manager"), &["/ws/common/shared.m"]),
            ],
        )
    }

    #[test]
    fn multimap_common_file_maps_to_two_targets() {
        let st = rvmac_state();
        let targets = st.file_to_targets.get(Path::new("/ws/common/shared.m")).unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn route_common_picks_deterministic_primary() {
        let st = rvmac_state();
        // Display sorts before Manager by project path → Display wins, every time.
        let key = st.route(Path::new("/ws/common/shared.m")).unwrap();
        assert_eq!(key.1, "Display");
    }

    #[test]
    fn route_project_file_picks_its_target() {
        let st = rvmac_state();
        let key = st
            .route(Path::new("/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay/main.m"))
            .unwrap();
        assert_eq!(key.1, "Display");
    }

    #[test]
    fn route_unknown_file_prefix_matches_project_dir() {
        let st = rvmac_state();
        // A brand-new file under Manager's project dir, not in any pbxproj.
        let key = st
            .route(Path::new("/ws/RVmacRXViewerManager/RVmacRXViewerManager/NewFile.m"))
            .unwrap();
        assert_eq!(key.1, "Manager");
    }

    #[test]
    fn scan_roots_scope_project_plus_common_excluding_siblings() {
        let st = rvmac_state();
        let d = "/ws/RVmacRXViewerDisplay/RVmacRXViewerDisplay.xcodeproj";
        let roots = st.scan_roots_for(&(d.to_string(), "Display".to_string()));
        // Display's own project dir + common, NOT Manager/Starter.
        assert!(roots.contains(&PathBuf::from("/ws/RVmacRXViewerDisplay")));
        assert!(roots.contains(&PathBuf::from("/ws/common")));
        assert!(!roots.contains(&PathBuf::from("/ws/RVmacRXViewerManager")));
        assert!(!roots.contains(&PathBuf::from("/ws/RVmacRXViewerStarter")));
    }
}
