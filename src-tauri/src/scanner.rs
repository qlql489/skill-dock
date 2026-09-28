//! Skill scanner — finds SKILL.md files inside a source root.
//!
//! Walks the tree with symlink-following + a visited set (keyed on the
//! canonicalized real path) to safely handle the common "central vault +
//! per-agent symlink" layout (e.g. `~/.claude/skills/foo -> ~/.agents/skills/foo`).
//! Algorithm mirrors multica's `enumerateLocalSkills`:
//!   - a directory containing `SKILL.md` is a skill — register and STOP
//!     descending that branch (never recurse into a directory that already
//!     qualifies as a skill);
//!   - a directory without `SKILL.md` is recursed into (so nested layouts
//!     like `category/sub/SKILL.md` are still discovered);
//!   - symlinks are resolved per level (`eval_symlinks` + `metadata`) and
//!     the real path is tracked to break loops.

use crate::state::{Skill, Source};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const SKILL_MD: &str = "SKILL.md";
/// Cap on recursion depth. Deep enough to cover nested layouts like
/// `category/sub/SKILL.md`, shallow enough that accidentally pointing at
/// `$HOME` won't walk the whole tree. Matches multica's maxLocalSkillDirDepth.
const MAX_DEPTH: usize = 4;
/// Hard cap on skills found — protects against accidentally pointing at a
/// giant tree. If a source has more than this, the scan is truncated and a
/// warning is logged.
const MAX_SKILLS_PER_SOURCE: usize = 5_000;

/// Directory names we always skip (case-insensitive). These are build/cache
/// dirs that never contain skills but can be enormous.
const SKIP_DIRS: &[&str] = &[
    ".git", ".hg", ".svn",
    // Backup copies of skills (e.g. `backups/wb-deploy-pre-<change>-<date>`)
    // contain a full SKILL.md, so without this they register as duplicate
    // same-name rows in the library.
    "backups",
    "node_modules", "bower_components",
    "target", "dist", "build", "out", ".next", ".nuxt", ".cache", ".parcel-cache",
    "__pycache__", ".venv", "venv", "env", ".tox",
    ".idea", ".vscode",
    ".gradle", ".terraform",
    "Library", // macOS user Library is huge
    "DerivedData",
];

pub fn should_skip_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIP_DIRS.iter().any(|s| s.to_lowercase() == lower)
}

/// Scan a source root and return all detected skills.
///
/// A "skill" is any directory that contains a SKILL.md file. We follow
/// symlinks (resolving the real path each level to break loops) and stop
/// descending into a directory once it qualifies as a skill.
pub fn scan(source: &Source, root: &Path, vault: &Path) -> Result<Vec<Skill>> {
    let mut skills = Vec::new();

    if !root.exists() {
        log::warn!("scan: root does not exist: {}", root.display());
        return Ok(skills);
    }
    log::info!(
        "scan: starting source={} name={} root={}",
        source.id,
        source.name,
        root.display()
    );
    let started = std::time::Instant::now();

    // Resolve the root itself through symlinks so a root that *is* a symlink
    // (e.g. lark-style) still walks correctly.
    let root_real = eval_real(root).unwrap_or_else(|| root.to_path_buf());
    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(root_real.clone());

    let mut truncated = false;
    enumerate(
        source,
        root,
        root,
        0,
        &mut visited,
        &mut skills,
        &mut truncated,
        vault,
    );

    skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let elapsed = started.elapsed();
    if truncated {
        log::warn!(
            "scan: source={} truncated at {} skills in {:?}",
            source.id, MAX_SKILLS_PER_SOURCE, elapsed
        );
    } else {
        log::info!(
            "scan: source={} found {} skills in {:?}",
            source.id,
            skills.len(),
            elapsed
        );
    }
    Ok(skills)
}

/// Recursive enumeration. `dir` is the directory to inspect, `root` is the
/// source root (for computing relative paths), `depth` is the current depth.
fn enumerate(
    source: &Source,
    dir: &Path,
    root: &Path,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<Skill>,
    truncated: &mut bool,
    vault: &Path,
) {
    if *truncated || out.len() >= MAX_SKILLS_PER_SOURCE {
        *truncated = true;
        return;
    }
    if depth > MAX_DEPTH {
        return;
    }

    // Does THIS directory itself contain a SKILL.md? If so it's a skill —
    // register it and do NOT descend further.
    let skill_md = dir.join(SKILL_MD);
    if skill_md.is_file() {
        if let Some(skill) = build_skill(source, dir, root, vault) {
            out.push(skill);
        }
        return;
    }

    // Otherwise enumerate children and recurse into subdirectories.
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log::debug!("scan: read_dir failed for {}: {e}", dir.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };

        // Skip dotfiles (mirrors multica isIgnoredLocalSkillEntry) — covers
        // .git, .config, .cache, archive dirs, etc.
        if name_str.starts_with('.') {
            continue;
        }
        // Skip known huge/build dirs.
        if should_skip_dir(name_str) {
            continue;
        }

        // Follow symlinks: metadata() (not symlink_metadata) resolves links,
        // so a symlinked directory reports as a directory.
        let child = entry.path();
        let meta = match std::fs::metadata(&child) {
            Ok(m) => m,
            Err(_) => continue, // dangling symlink or permission issue — skip
        };
        if !meta.is_dir() {
            continue;
        }

        // Resolve the real path to break symlink loops.
        let real = match eval_real(&child) {
            Some(r) => r,
            None => continue,
        };
        if !visited.insert(real) {
            // Already walked this real directory — skip to avoid a loop.
            continue;
        }

        enumerate(source, &child, root, depth + 1, visited, out, truncated, vault);
    }
}

/// Build a Skill record from a directory known to contain SKILL.md.
fn build_skill(source: &Source, skill_dir: &Path, root: &Path, vault: &Path) -> Option<Skill> {
    let skill_md_path = skill_dir.join(SKILL_MD);

    let relative = skill_dir
        .strip_prefix(root)
        .unwrap_or(skill_dir)
        .to_path_buf();

    let (name, description) = parse_frontmatter(&skill_md_path);
    let fallback_name = dir_name_fallback(skill_dir, root);

    let modified_at = file_modified(&skill_md_path).unwrap_or_else(Utc::now);
    let size_bytes = skill_size(skill_dir).unwrap_or(0);
    let content_hash = blake3::hash(
        &std::fs::read(&skill_md_path).unwrap_or_default(),
    )
    .to_hex()
    .to_string();

    let is_shared = is_shared_skill(skill_dir, vault);

    // Detect whether the skill directory itself is a symlink (as opposed to a
    // real folder). `symlink_metadata` does not follow the link, so its
    // `file_type().is_symlink()` tells us if this is one.
    let (is_symlink, symlink_target) = match std::fs::symlink_metadata(skill_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(skill_dir).ok();
            (true, target)
        }
        _ => (false, None),
    };

    let id = format!("{}:{}", source.id, relative.display());

    Some(Skill {
        id,
        source_id: source.id,
        name: if name.is_empty() { fallback_name } else { name },
        description,
        relative_path: relative,
        absolute_path: skill_dir.to_path_buf(),
        has_skill_md: true,
        content_hash,
        modified_at,
        size_bytes,
        is_shared,
        is_symlink,
        symlink_target,
    })
}

/// Resolve a path to its canonical real path, following all symlinks.
/// Returns None if resolution fails (e.g. dangling link).
fn eval_real(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// Lightweight size: sum of the SKILL.md and its sibling files (non-recursive).
/// We intentionally do NOT walk the whole skill subtree — that was a major
/// source of slow IO (a second full WalkDir per skill) and double-counted
/// bytes when the skill dir contained symlinked children.
pub(crate) fn skill_size(dir: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        if let Ok(m) = entry.metadata() {
            if m.is_file() {
                total += m.len();
            }
        }
    }
    Ok(total)
}

/// A skill is "shared" if its real (canonicalized) location lives under the
/// central vault（默认应用数据目录下的 `skills`，见 discover::effective_vault_dir）.
/// This lets the UI flag skills that multiple agents see via symlinks back to
/// the same vault.
fn is_shared_skill(skill_dir: &Path, vault: &Path) -> bool {
    let real = match eval_real(skill_dir) {
        Some(r) => r,
        None => return false,
    };
    if let Ok(rest) = real.strip_prefix(vault) {
        // real path is under <vault>/<something>
        return !rest.as_os_str().is_empty();
    }
    false
}

fn dir_name_fallback(dir: &Path, root: &Path) -> String {
    let rel = dir.strip_prefix(root).unwrap_or(dir);
    rel.iter()
        .last()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.display().to_string())
}

pub(crate) fn file_modified(path: &Path) -> Option<DateTime<Utc>> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some(DateTime::<Utc>::from(mtime))
}

/// Parses a simple YAML frontmatter from the top of SKILL.md and pulls
/// `name` and `description`. Avoids pulling in a full YAML lib.
pub(crate) fn parse_frontmatter(path: &Path) -> (String, String) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (String::new(), String::new());
    };
    let mut lines = content.lines();
    if lines.next().map(|l| l.trim()) != Some("---") {
        return (String::new(), String::new());
    }

    let mut name = String::new();
    let mut description = String::new();

    for line in lines.by_ref() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            name = rest.trim().trim_matches(['"', '\'']).to_string();
        } else if let Some(rest) = trimmed.strip_prefix("description:") {
            description = rest.trim().trim_matches(['"', '\'']).to_string();
        }
    }
    (name, description)
}

#[allow(dead_code)]
fn _ensure_compiles() -> Result<()> {
    let _ = SKILL_MD;
    let _: PathBuf = PathBuf::new();
    let _: anyhow::Error = anyhow::anyhow!("x");
    let _ = WalkDir::new("");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{CloneStatus, SourceKind, SourceMode};
    use uuid::Uuid;

    fn tmpdir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("sm-scanner-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn test_source(root: &Path) -> Source {
        Source {
            id: Uuid::new_v4(),
            kind: SourceKind::Local,
            name: "test".into(),
            location: root.display().to_string(),
            clone_path: None,
            branch: None,
            created_at: Utc::now(),
            last_scanned_at: None,
            last_commit_sha: None,
            skill_count: 0,
            clone_status: CloneStatus::Pending,
            clone_error: None,
            mode: SourceMode::Flat,
            auto_sync_targets: vec![],
        }
    }

    #[test]
    fn scan_skips_backup_dirs() {
        // backups/ 下的副本也带 SKILL.md，若不跳过会在库里出现同名重复行。
        let root = tmpdir("backups");
        let real = root.join("skills/wb-deploy");
        let backup = root.join("backups/wb-deploy-pre-integration-20260910");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(real.join(SKILL_MD), "---\nname: wb-deploy\ndescription: real\n---\n").unwrap();
        std::fs::write(backup.join(SKILL_MD), "---\nname: wb-deploy\ndescription: backup copy\n---\n").unwrap();

        let src = test_source(&root);
        let found = scan(&src, &root, &root).unwrap();
        let paths: Vec<_> = found.iter().map(|s| s.relative_path.clone()).collect();
        assert_eq!(
            paths,
            vec![PathBuf::from("skills/wb-deploy")],
            "backups/ 下的技能副本不应被注册"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
