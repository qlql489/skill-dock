//! Symlink-based install/uninstall to target agent directories.
//!
//! ## Safety model — "never destroy what we didn't create"
//!
//! Before touching anything at a would-be link path we classify what's there:
//!
//! - **Absent** → safe to create.
//! - **Symlink + an installation record vouches for this (skill,target) pair**
//!   → it's ours. If it still points at this skill, install is an idempotent
//!   no-op; if it points somewhere stale (the source moved), we silently
//!   repair it. Deletion is allowed via [`uninstall`].
//! - **Symlink with no record** → someone else's link. Only replaced when the
//!   caller explicitly passes [`ConflictPolicy::Replace`] AND only the link
//!   itself is removed (never any target of the link).
//! - **Real directory or real file** → never touched by ANY policy. Install
//!   refuses with a clear error naming the path so the user can decide.

use crate::state::{Installation, PersistedState, Skill};
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};

/// What to do when the target link path already exists and is not ours.
#[derive(Debug, Clone, Copy)]
pub enum ConflictPolicy {
    /// Fail with an error.
    Fail,
    /// Replace an existing *symlink* (never a real directory/file).
    Replace,
}

/// What currently occupies a would-be link path on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Occupant {
    Absent,
    /// A symlink whose stored destination is attached (read_link result).
    Symlink(Option<PathBuf>),
    RealDir,
    RealFile,
}

/// Classify WITHOUT following symlinks, so a managed link that was swapped
/// for a real folder is seen as a real folder — not as "our content".
pub fn classify(path: &Path) -> Occupant {
    match std::fs::symlink_metadata(path) {
        Err(_) => Occupant::Absent,
        Ok(m) if m.file_type().is_symlink() => {
            let dest = std::fs::read_link(path).ok();
            Occupant::Symlink(dest)
        }
        Ok(m) if m.is_dir() => Occupant::RealDir,
        Ok(_) => Occupant::RealFile,
    }
}

/// True when `a` and `b` refer to the same directory. Lexical compare first,
/// then canonicalized compare for paths that both resolve (case-insensitive
/// filesystems, `..` segments, etc.).
fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

pub fn install(
    state: &mut PersistedState,
    skill: &Skill,
    target_id: &str,
    policy: ConflictPolicy,
) -> Result<Installation> {
    let target = state
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .cloned()
        .context("target not found")?;

    if !target.enabled {
        anyhow::bail!("target '{}' is disabled", target.name);
    }

    // Installs always land in the DEFAULT dir (element 0 of all_dirs); extra
    // dirs are observation-only — their content is scanned and adoptable but
    // never written by install.
    std::fs::create_dir_all(&target.skills_dir)
        .with_context(|| format!("create {}", target.skills_dir.display()))?;

    let link_name = skill_name_for_link(skill);
    let link_path = target.skills_dir.join(&link_name);

    // Does OUR ledger vouch for this exact slot? reconcile() keeps records in
    // sync with reality, so a record here means "we created this name before".
    let owned_record = state
        .installations
        .iter()
        .any(|i| i.skill_id == skill.id && i.target_id == target_id);

    let desired = skill.absolute_path.clone();

    match classify(&link_path) {
        Occupant::Absent => create_symlink(&link_path, &desired)?,

        Occupant::Symlink(current) => {
            let points_here = current.as_deref().map(|c| same_dir(c, &desired)).unwrap_or(false);
            if owned_record {
                if points_here {
                    // Idempotent re-install: keep original installed_at.
                    return Ok(record_installation(state, skill, target_id, link_path, false));
                }
                // Ours but stale (source moved / repointed): repair is always
                // allowed — record ownership trumps the conflict policy.
                std::fs::remove_file(&link_path)
                    .with_context(|| format!("stale link at {}", link_path.display()))?;
                create_symlink(&link_path, &desired)?;
            } else {
                match policy {
                    ConflictPolicy::Fail => anyhow::bail!(
                        "已被占用：{}（指向 {}）",
                        link_path.display(),
                        current.map(|c| c.display().to_string()).unwrap_or_else(|| "?".into())
                    ),
                    ConflictPolicy::Replace => {
                        // Explicit user intent — replacing a link is fine;
                        // read_link output was never our data to begin with.
                        std::fs::remove_file(&link_path)?;
                        create_symlink(&link_path, &desired)?;
                    }
                }
            }
        }

        occupant @ (Occupant::RealDir | Occupant::RealFile) => {
            anyhow::bail!(
                "{} 处已存在{}「{}」，不是本程序创建的软链，拒绝覆盖。请手动处理或为 skill 改名。",
                link_path.display(),
                if matches!(occupant, Occupant::RealDir) { "目录" } else { "文件" },
                link_name,
            );
        }
    }

    Ok(record_installation(state, skill, target_id, link_path, true))
}

/// Insert or refresh the installation record. When `touch` is false and a
/// record exists, its original `installed_at` is preserved.
fn record_installation(
    state: &mut PersistedState,
    skill: &Skill,
    target_id: &str,
    link_path: PathBuf,
    touch: bool,
) -> Installation {
    match state.installations.iter_mut().find(|i| {
        i.skill_id == skill.id && i.target_id == target_id && i.link_path == link_path
    }) {
        Some(existing) => {
            existing.status = "ok".to_string();
            if touch {
                existing.installed_at = Utc::now();
            }
            existing.clone()
        }
        None => {
            // Same (skill,target) under an old path (rename case)? Drop it.
            state.installations.retain(|i| !(i.skill_id == skill.id && i.target_id == target_id));
            let inst = Installation {
                skill_id: skill.id.clone(),
                target_id: target_id.to_string(),
                link_path,
                installed_at: Utc::now(),
                status: "ok".to_string(),
            };
            state.installations.push(inst.clone());
            inst
        }
    }
}

fn create_symlink(link_path: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(dest, link_path).with_context(|| {
        format!(
            "create symlink {} -> {}",
            link_path.display(),
            dest.display()
        )
    })
}

pub fn uninstall(state: &mut PersistedState, skill_id: &str, target_id: &str) -> Result<()> {
    let inst = state
        .installations
        .iter()
        .find(|i| i.skill_id == skill_id && i.target_id == target_id)
        .cloned()
        .context("installation not found")?;

    if inst.link_path.symlink_metadata().is_ok() {
        let meta = std::fs::symlink_metadata(&inst.link_path)?;
        if meta.file_type().is_symlink() {
            std::fs::remove_file(&inst.link_path)?;
        } else {
            anyhow::bail!("refusing to remove non-symlink: {}", inst.link_path.display());
        }
    }

    state.installations.retain(|i| !(i.skill_id == skill_id && i.target_id == target_id));
    Ok(())
}

/// After a target's skills_dir change, carry its managed links over to the
/// new directory. Only entries in OUR ledger are touched, and only symlinks
/// are ever removed — a real dir/file at either end stays put (safety model).
/// Ledger entries are re-pointed to the new location either way, so
/// reconcile() reports truthfully ("missing" until the skill actually exists
/// in the new dir).
///
/// Returns `(moved, left)`: moved = symlink recreated at the new path and the
/// old link removed; left = ledger re-pointed without a filesystem move (old
/// occupant absent/not a link, or the new slot already occupied).
pub fn migrate_target_links(
    state: &mut PersistedState,
    target_id: &str,
    old_dir: &Path,
    new_dir: &Path,
) -> Result<(usize, usize)> {
    // Plan immutably (filesystem classification + ledger read), apply after.
    let plan: Vec<(PathBuf, PathBuf, Option<PathBuf>)> = state
        .installations
        .iter()
        .filter(|i| i.target_id == target_id && i.link_path.starts_with(old_dir))
        .map(|i| {
            let rel = i
                .link_path
                .strip_prefix(old_dir)
                .unwrap_or_else(|_| i.link_path.file_name().map(|n| Path::new(n)).unwrap_or(Path::new("")));
            let new_path = new_dir.join(rel);
            // Resolve now (absolute, or relative against the OLD link's parent)
            // so the symlink recreated at the new location points correctly.
            let dest = match classify(&i.link_path) {
                Occupant::Symlink(Some(d)) => {
                    let resolved = if d.is_absolute() {
                        d
                    } else {
                        i.link_path
                            .parent()
                            .unwrap_or(old_dir)
                            .join(d)
                    };
                    Some(resolved)
                }
                _ => None,
            };
            (i.link_path.clone(), new_path, dest)
        })
        .collect();

    let mut moved = 0;
    let mut left = 0;
    for (old_path, new_path, dest) in plan {
        let mut did_move = false;
        if let Some(dest) = dest {
            match classify(&new_path) {
                Occupant::Absent => {
                    if let Some(parent) = new_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    create_symlink(&new_path, &dest)?;
                    did_move = true;
                }
                Occupant::Symlink(current) => {
                    // Already in place at the new location (re-point without a
                    // dir change earlier?) — idempotent, still retire the old.
                    if current.as_deref().map(|c| same_dir(c, &dest)).unwrap_or(false) {
                        did_move = true;
                    }
                    // A different link occupies the new slot → move nothing;
                    // the ledger re-point below surfaces it honestly.
                }
                Occupant::RealDir | Occupant::RealFile => {}
            }
            if did_move {
                if let Occupant::Symlink(_) = classify(&old_path) {
                    std::fs::remove_file(&old_path)
                        .with_context(|| format!("remove old link {}", old_path.display()))?;
                }
            }
        }
        if did_move {
            moved += 1;
        } else {
            left += 1;
        }
        if let Some(rec) = state
            .installations
            .iter_mut()
            .find(|i| i.target_id == target_id && i.link_path == old_path)
        {
            rec.link_path = new_path;
        }
    }
    Ok((moved, left))
}

/// Re-verify every installation record against disk and TAG its health
/// instead of silently dropping rows (`missing` / `conflict` stay visible to
/// the overview page so the user can act on them).
///
/// Returns the number of records whose status changed.
pub fn reconcile(state: &mut PersistedState) -> Result<usize> {
    let mut changed = 0usize;
    for inst in state.installations.iter_mut() {
        let new_status = match classify(&inst.link_path) {
            Occupant::Absent => "missing",
            Occupant::Symlink(_) => {
                // A live symlink named what we recorded — but does it point at
                // the right skill? A repointed link is surfaced as a conflict
                // (discover_target_skills reports the same thing on disk).
                let still_valid = state.skills.get(&inst.skill_id).map(|s| {
                    std::fs::read_link(&inst.link_path)
                        .map(|dest| same_dir(&dest, &s.absolute_path))
                        .unwrap_or(false)
                });
                match still_valid {
                    Some(true) | None => "ok",
                    Some(false) => "conflict",
                }
            }
            _ => "conflict",
        };
        if inst.status != new_status {
            inst.status = new_status.to_string();
            changed += 1;
        }
    }
    // Re-verify only tags records we already have; this discovers links the
    // ledger has never heard of (hand-made / other tooling) so a source added
    // after the links were made still shows its installs.
    changed += adopt_external_links(state);
    if changed > 0 {
        log::info!("reconcile: {changed} installation change(s)");
    }
    Ok(changed)
}

/// Walk every target's skills_dir and ADOPT symlinks that point at a known
/// skill but have no installation record — links created by other tooling or
/// by hand (e.g. wb-cli linking skills into ~/.agents/skills before the source
/// was ever registered here). Without this, a freshly-added source shows all
/// its skills as 未安装 and a manual install dies on the 已被占用 conflict.
///
/// Matching is by the link's RESOLVED real path against each skill's
/// canonical absolute_path, so link names may differ from skill names. Only
/// symlinks are adopted; real dirs/files and dangling links are never touched.
/// state.skills is a BTreeMap, so on duplicate real paths first-wins is
/// deterministic. Returns the number of records created.
pub fn adopt_external_links(state: &mut PersistedState) -> usize {
    let mut by_real: std::collections::HashMap<PathBuf, String> = Default::default();
    for (id, skill) in &state.skills {
        if let Ok(real) = std::fs::canonicalize(&skill.absolute_path) {
            by_real.entry(real).or_insert_with(|| id.clone());
        }
    }
    if by_real.is_empty() {
        return 0;
    }

    let mut adopted = 0usize;
    for target in &state.targets {
        // 默认目录 + 额外地址都参与收编；额外目录里的外部软链同样入账。
        let mut entries = Vec::new();
        for dir in crate::targets::all_dirs(target) {
            match std::fs::read_dir(&dir) {
                Ok(e) => entries.extend(e.flatten()),
                Err(_) => continue, // dir absent (disabled / never launched) — fine
            }
        }
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let link_path = entry.path();
            if !matches!(classify(&link_path), Occupant::Symlink(_)) {
                continue;
            }
            // Dangling links don't canonicalize; a link to a file inside a
            // skill dir resolves to a path that's not in the map — both skip.
            let Ok(real) = std::fs::canonicalize(&link_path) else {
                continue;
            };
            let Some(skill_id) = by_real.get(&real) else {
                continue;
            };
            // One record per (skill, target): extra links to the same skill
            // (alias names) are covered by the first one.
            if state
                .installations
                .iter()
                .any(|i| i.skill_id == *skill_id && i.target_id == target.id)
            {
                continue;
            }
            state.installations.push(Installation {
                skill_id: skill_id.clone(),
                target_id: target.id.clone(),
                link_path,
                installed_at: Utc::now(),
                status: "ok".to_string(),
            });
            adopted += 1;
        }
    }
    if adopted > 0 {
        log::info!("adopt: recognized {adopted} externally-created link(s) as installations");
    }
    adopted
}

/// Delete installation records whose status is one of the given filters
/// ("missing" / "conflict"). Used by the overview page's 清理 button.
pub fn drop_statuses(state: &mut PersistedState, statuses: &[&str]) -> usize {
    let before = state.installations.len();
    state.installations.retain(|i| !statuses.contains(&i.status.as_str()));
    before - state.installations.len()
}

/// What we name the symlink inside the target dir. Prefer the
/// skill's display name (from frontmatter) but ensure it's filesystem-safe
/// and unique enough.
fn skill_name_for_link(skill: &Skill) -> String {
    let mut s = skill
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect::<String>();
    if s.is_empty() {
        s = skill
            .relative_path
            .iter()
            .last()
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_else(|| skill.id.clone());
    }
    s
}

/// Public re-export for callers that need to predict where a skill's link
/// would land without installing it (preview/classification).
pub fn link_name_for(skill: &Skill) -> String {
    skill_name_for_link(skill)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn tmpdir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("sm-symlinks-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_skill(dir: &Path) -> Skill {
        Skill {
            id: format!("test-source:{}", dir.file_name().unwrap().to_string_lossy()),
            source_id: uuid::Uuid::new_v4(),
            name: dir.file_name().unwrap().to_string_lossy().to_string(),
            description: String::new(),
            relative_path: dir.file_name().unwrap().into(),
            absolute_path: dir.to_path_buf(),
            has_skill_md: true,
            content_hash: String::new(),
            modified_at: Utc::now(),
            size_bytes: 0,
            is_shared: false,
            is_symlink: false,
            symlink_target: None,
        }
    }

    fn make_state(target_dir: &Path, skills: &[Skill]) -> PersistedState {
        let mut s = PersistedState::default();
        s.targets.push(crate::state::AgentTarget {
            id: "t1".into(),
            name: "Target".into(),
            skills_dir: target_dir.to_path_buf(),
            extra_dirs: Vec::new(),
            enabled: true,
            description: String::new(),
            tag: None,
            detected: true,
            user_touched: false,
        });
        for sk in skills {
            s.skills.insert(sk.id.clone(), sk.clone());
        }
        s
    }

    #[cfg(unix)]
    #[test]
    fn real_dir_blocks_install_even_with_replace_policy() {
        let root = tmpdir("realdir");
        let target_dir = root.join("target");
        let skill_src = root.join("myskill");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::create_dir_all(&skill_src).unwrap();

        // 用户自己的目录，里面有人类写的数据。
        let user_data = target_dir.join("myskill");
        std::fs::create_dir_all(&user_data).unwrap();
        std::fs::write(user_data.join("precious.txt"), "do not lose me").unwrap();

        let skill = make_skill(&skill_src);
        let mut st = make_state(&target_dir, &[skill.clone()]);

        let err = install(&mut st, &skill, "t1", ConflictPolicy::Replace).unwrap_err();
        assert!(format!("{err:#}").contains("拒绝覆盖"), "got: {err:#}");
        // 数据必须原封不动。
        assert_eq!(
            std::fs::read_to_string(user_data.join("precious.txt")).unwrap(),
            "do not lose me"
        );
        assert!(st.installations.is_empty(), "失败时不得留下安装记录");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn foreign_symlink_needs_explicit_replace() {
        let root = tmpdir("foreign");
        let target_dir = root.join("target");
        let other = root.join("other");
        let mine = root.join("mine");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&mine).unwrap();

        let link = target_dir.join("mine");
        std::os::unix::fs::symlink(&other, &link).unwrap();

        let skill = make_skill(&mine);
        let mut st = make_state(&target_dir, &[skill.clone()]);

        // Fail：拒绝，且别人的链接保留。
        assert!(install(&mut st, &skill, "t1", ConflictPolicy::Fail).is_err());
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::read_link(&link).unwrap().ends_with("other"));

        // Replace（用户显式勾选）：允许顶掉"链接本身"，指向的目标不动。
        install(&mut st, &skill, "t1", ConflictPolicy::Replace).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), mine);
        assert!(other.is_dir(), "被替换软链的真实目录必须完好");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn owned_stale_link_is_repaired_silently() {
        let root = tmpdir("stale");
        let target_dir = root.join("target");
        let old_loc = root.join("old-loc");
        let new_loc = root.join("new-loc");
        std::fs::create_dir_all(&target_dir).unwrap();
        for d in [&old_loc, &new_loc] {
            std::fs::create_dir_all(d).unwrap();
        }

        let mut skill = make_skill(&new_loc);
        let mut st = make_state(&target_dir, &[skill.clone()]);
        // 名字改一下避免两目录同名冲突？不需要：same_dir 比较 canonicalize。
        skill.absolute_path = new_loc.clone();

        // 链接还指着老位置 —— 记录在案，应自动修复。
        let link = target_dir.join("new-loc");
        std::os::unix::fs::symlink(&old_loc, &link).unwrap();
        st.installations.push(Installation {
            skill_id: skill.id.clone(),
            target_id: "t1".into(),
            link_path: link.clone(),
            installed_at: Utc::now(),
            status: String::new(),
        });

        // Fail 策略也要能修复（自有记录优先于策略）。
        install(&mut st, &skill, "t1", ConflictPolicy::Fail).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), new_loc);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_marks_missing_and_conflict_instead_of_dropping() {
        let root = tmpdir("reconcile");
        let target_dir = root.join("target");
        let src = root.join("src");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::create_dir_all(&src).unwrap();

        // missing：链接已被人删除。
        let gone = target_dir.join("gone");
        // conflict：名字被换成了真目录。
        let clob = target_dir.join("clob");
        std::fs::create_dir_all(&clob).unwrap();
        // ok：健在。
        let alive = target_dir.join("alive");
        std::os::unix::fs::symlink(&src, &alive).unwrap();

        let mut st = PersistedState::default();
        st.targets.push(crate::state::AgentTarget {
            id: "t1".into(),
            name: "T".into(),
            skills_dir: target_dir.clone(),
            extra_dirs: Vec::new(),
            enabled: true,
            description: String::new(),
            tag: None,
            detected: true,
            user_touched: false,
        });
        for (name, sid) in [("gone", "s-gone"), ("clob", "s-clob"), ("alive", "s-alive")] {
            st.installations.push(Installation {
                skill_id: sid.into(),
                target_id: "t1".into(),
                link_path: target_dir.join(name),
                installed_at: Utc::now(),
                status: "ok".into(),
            });
        }
        st.skills.insert(
            "s-alive".into(),
            make_skill(&src),
        );
        let _ = BTreeMap::<String, ()>::new();

        reconcile(&mut st).unwrap();
        let by_skill: std::collections::HashMap<_, _> = st
            .installations
            .iter()
            .map(|i| (i.skill_id.as_str(), i.status.as_str()))
            .collect();
        assert_eq!(by_skill["s-gone"], "missing");
        assert_eq!(by_skill["s-clob"], "conflict");
        assert_eq!(by_skill["s-alive"], "ok");
        assert_eq!(st.installations.len(), 3, "记录不应被丢弃");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_adopts_external_links_pointing_at_known_skills() {
        let root = tmpdir("adopt");
        let target_dir = root.join("target");
        let skill_src = root.join("src").join("wb-foo");
        std::fs::create_dir_all(&skill_src).unwrap();
        std::fs::create_dir_all(&target_dir).unwrap();

        // 外部工具（wb-cli 等）在来源注册之前就建好的链接。
        let link = target_dir.join("wb-foo");
        std::os::unix::fs::symlink(&skill_src, &link).unwrap();
        // 不收编：指向未知目录的链接。
        let stranger = root.join("stranger");
        std::fs::create_dir_all(&stranger).unwrap();
        std::os::unix::fs::symlink(&stranger, target_dir.join("stranger")).unwrap();
        // 不收编：悬空链接。
        std::os::unix::fs::symlink(&root.join("nope"), target_dir.join("dangling")).unwrap();
        // 不收编：真目录占位。
        std::fs::create_dir_all(target_dir.join("realdir")).unwrap();

        let skill = make_skill(&skill_src);
        let mut st = make_state(&target_dir, &[skill]);

        let n = reconcile(&mut st).unwrap();
        assert_eq!(n, 1, "只应收编指向已知 skill 的那一条链接");
        let inst = &st.installations[0];
        assert_eq!(inst.skill_id, "test-source:wb-foo");
        assert_eq!(inst.target_id, "t1");
        assert_eq!(inst.status, "ok");
        assert_eq!(inst.link_path, link);

        // 幂等：重复 reconcile 不再新增；别名链接也被 (skill,target) 去重挡住。
        std::os::unix::fs::symlink(&skill_src, target_dir.join("alias")).unwrap();
        assert_eq!(reconcile(&mut st).unwrap(), 0);
        assert_eq!(st.installations.len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn target_dir_change_migrates_owned_links_and_spares_real_dirs() {
        let root = tmpdir("migrate");
        let old_dir = root.join("old-target");
        let new_dir = root.join("new-target");
        let skill_src = root.join("myskill");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::create_dir_all(&skill_src).unwrap();

        let skill = make_skill(&skill_src);
        let mut st = make_state(&old_dir, &[skill.clone()]);
        let inst = install(&mut st, &skill, "t1", ConflictPolicy::Fail).unwrap();
        assert_eq!(inst.link_path, old_dir.join("myskill"));

        // 旧位置另一个名字被真实目录占着（有台账）：迁移时必须原样保留。
        let real_dir_entry = old_dir.join("handmade");
        std::fs::create_dir_all(&real_dir_entry).unwrap();
        std::fs::write(real_dir_entry.join("data.txt"), "keep").unwrap();
        st.installations.push(Installation {
            skill_id: "test-source:handmade".into(),
            target_id: "t1".into(),
            link_path: real_dir_entry.clone(),
            installed_at: Utc::now(),
            status: "ok".into(),
        });

        let (moved, left) = migrate_target_links(&mut st, "t1", &old_dir, &new_dir).unwrap();
        assert_eq!((moved, left), (1, 1));

        // 软链：新位置重建并指向原目标，旧链接已删，台账改指新路径。
        let new_link = new_dir.join("myskill");
        assert!(std::fs::symlink_metadata(&new_link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&new_link).unwrap(), skill_src);
        assert!(std::fs::symlink_metadata(&old_dir.join("myskill")).is_err(), "旧软链应已删除");

        // 真实目录：新位置不出现、旧位置原封不动，台账只改指向（reconcile 会如实报 missing）。
        assert!(!new_dir.join("handmade").exists());
        assert_eq!(
            std::fs::read_to_string(real_dir_entry.join("data.txt")).unwrap(),
            "keep"
        );
        let ledger: Vec<(String, bool)> = st
            .installations
            .iter()
            .map(|i| (i.skill_id.clone(), i.link_path.starts_with(&new_dir)))
            .collect();
        assert!(ledger.contains(&("test-source:myskill".into(), true)));
        assert!(ledger.contains(&("test-source:handmade".into(), true)));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
