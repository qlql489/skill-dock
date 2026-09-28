//! 压缩包（.zip / .skill）导入。
//!
//! 安全两条铁律：
//! 1. **Zip-slip 防护** —— 每个条目的解压路径必须落在目标根目录之内
//!    （拒绝绝对路径和 `..` 上跳），否则跳过并记录警告。
//! 2. **符号链接一律不落地** —— 压缩包内的 symlink 条目直接跳过，
//!    防止解压后指向任意系统路径的逃逸链。

use crate::state::Source;
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 单包总量上限：512 MB。超过即中止导入（防 zip 炸弹朴素兜底）。
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// 条目数上限。
const MAX_ENTRIES: usize = 20_000;

pub struct ExtractOutcome {
    /// 解压根（可能被 collapse_single_root 降了一层）。
    pub dest_dir: PathBuf,
    /// 实际创建的解压根目录（repos/zips/<uuid>，清理用）。
    pub extract_root: PathBuf,
    pub extracted_files: usize,
    pub skipped_symlinks: usize,
}

/// 把压缩包安全地解压到应用数据目录的 `repos/zips/<uuid>/`。
pub fn extract_archive(zip_path: &Path) -> Result<ExtractOutcome> {
    if !zip_path.is_file() {
        bail!("不是文件: {}", zip_path.display());
    }

    let dest_dir = crate::paths::repos_dir()?.join("zips").join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("create {}", dest_dir.display()))?;

    let file = std::fs::File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("读取压缩包失败")?;

    if archive.len() > MAX_ENTRIES {
        bail!("压缩包含 {} 个条目，超过上限 {MAX_ENTRIES}", archive.len());
    }

    let mut total_bytes: u64 = 0;
    let mut extracted_files = 0usize;
    let mut skipped_symlinks = 0usize;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("读取条目失败")?;

        // --- Zip-slip check: 展开后的路径必须还在 dest_dir 内 ---
        let Some(rel) = entry.enclosed_name() else {
            log::warn!("zip import: skipped unsafe path entry {:?}", entry.name());
            continue;
        };
        // macOS 的归档常带 __MACOSX 元数据目录，跳过噪音。
        let rel_str = rel.to_string_lossy();
        if rel_str.starts_with("__MACOSX") || rel_str.contains("/__MACOSX") || rel_str.ends_with(".DS_Store") {
            continue;
        }

        if entry.is_dir() {
            std::fs::create_dir_all(dest_dir.join(&rel))?;
            continue;
        }
        #[cfg(unix)]
        {
            if let Some(mode) = entry.unix_mode() {
                if mode & 0o170000 == 0o120000 {
                    // S_IFLNK — symlink entry stored as file content.
                    skipped_symlinks += 1;
                    log::warn!("zip import: skipped symlink entry {:?}", entry.name());
                    continue;
                }
            }
        }

        total_bytes += entry.size();
        if total_bytes > MAX_TOTAL_BYTES {
            let _ = std::fs::remove_dir_all(&dest_dir);
            bail!("解压总量超过 512MB，已中止");
        }

        let out_path = dest_dir.join(&rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;
        extracted_files += 1;
    }

    if extracted_files == 0 {
        let _ = std::fs::remove_dir_all(&dest_dir);
        bail!("压缩包里没有可导入的内容");
    }

    // 解压根就是 source 根：如果压缩包顶层只有一个子目录，把根降到那层，
    // 扫描出来的 relative_path 会更干净。
    let extract_root_snapshot = dest_dir.clone();
    let scan_root = collapse_single_root(dest_dir);

    Ok(ExtractOutcome {
        extract_root: extract_root_snapshot,
        dest_dir: scan_root,
        extracted_files,
        skipped_symlinks,
    })
}

/// 若根目录只含一个子目录（且没有散落文件），返回那个子目录作为根。
fn collapse_single_root(root: PathBuf) -> PathBuf {
    let Ok(entries) = std::fs::read_dir(&root) else { return root };
    let visible: Vec<_> = entries.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).collect();
    if visible.len() == 1 && visible[0].path().is_dir() {
        return visible[0].path();
    }
    root
}

/// 注册一个来自压缩包的本地来源。文件已经安全落盘，之后它就是一个普通
/// 的本地 folder 来源（享受 watcher 自动重扫）。
pub fn register_zip_source(
    state: &mut crate::state::PersistedState,
    display_name: String,
    root: &Path,
) -> Result<Source> {
    crate::sources::add_local(state, display_name, root.display().to_string(), crate::state::SourceMode::Flat, Vec::new())
}

/// 根目录直接躺着 SKILL.md —— 这不是"技能集合"，是单个技能
/// （collapse_single_root 已把根降到技能目录那一层）。
fn is_single_skill(root: &Path) -> bool {
    root.join("SKILL.md").is_file()
}

/// 中央仓库里的重名避让：business-overview → business-overview-2 → -3…
/// 与收编（adopt_local_skill）同一口径。
fn next_available_name(vault: &Path, base: &str) -> PathBuf {
    let mut dest = vault.join(base);
    let mut n = 2;
    while dest.exists() {
        dest = vault.join(format!("{base}-{n}"));
        n += 1;
    }
    dest
}

/// 单技能压缩包落地：技能目录挪进中央仓库，返回（必要时注册）「中央仓库」来源。
/// 顺序刻意为先锁内注册来源、后做文件 IO —— 万一挪文件失败，来源已可见、
/// 不至于技能进了仓库却无人认领。锁纪律：save 由调用方在锁外做。
fn install_single_skill_into_vault(
    state: &crate::state::AppState,
    skill_dir: &Path,
    skill_name: &str,
) -> Result<Source> {
    let vault = crate::discover::effective_vault_dir(&state.snapshot().ui.vault_path)
        .context("无法确定主目录")?;
    std::fs::create_dir_all(&vault).with_context(|| format!("create {}", vault.display()))?;

    // location == vault 的本地来源就是「中央仓库」（与收编同一判定）。
    let src = crate::discover::find_or_register_vault_source(state, &vault)
        .map_err(anyhow::Error::msg)?;

    // 同盘 rename（瞬完成）；跨盘回落到过滤复制。解压根由调用方清理。
    let dest = next_available_name(&vault, skill_name);
    if std::fs::rename(skill_dir, &dest).is_err() {
        crate::discover::copy_dir_filtered(skill_dir, &dest).map_err(anyhow::Error::msg)?;
    }
    log::info!("zip import: single skill -> vault {}", dest.display());

    Ok(src)
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

use crate::state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// 导入压缩包主流程（路径版与拖拽字节版共用）：安全解压 →
/// 单技能落中央仓库 / 多技能注册本地来源 → 后台扫描。
fn import_zip_file(
    display_name: String,
    zip_path: &Path,
    state: &Arc<AppState>,
    app: &AppHandle,
) -> Result<Source> {
    let outcome = extract_archive(zip_path)?;

    if outcome.skipped_symlinks > 0 {
        log::warn!("zip import: skipped {} symlink entr(ies)", outcome.skipped_symlinks);
    }

    let src = if is_single_skill(&outcome.dest_dir) {
        // 单技能包：技能名用解压出的目录名（未折叠时那是 uuid，退回压缩包名）。
        let skill_name = if outcome.dest_dir != outcome.extract_root {
            outcome
                .dest_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| display_name.clone())
        } else {
            display_name.clone()
        };
        let src = install_single_skill_into_vault(state, &outcome.dest_dir, &skill_name)?;
        // 技能已挪走，解压根（uuid 层）没有留着的理由。
        let _ = std::fs::remove_dir_all(&outcome.extract_root);
        src
    } else {
        {
            let mut guard = state.inner.write();
            register_zip_source(&mut guard, display_name, &outcome.dest_dir)?
        } // release write lock before save
    };
    state.save()?;

    log::info!(
        "zip import: registered {} ({} files) — scheduling scan",
        src.id,
        outcome.extracted_files
    );
    let state_for_thread = state.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        crate::commands::run_scan_blocking(&app, &state_for_thread, src.id, "zip-import".to_string());
        let _ = app.emit("skill-dock://source-scanned", src.id.to_string());
    });
    Ok(src)
}

/// 导入压缩包：安全解压 → 注册为本地来源 → 后台扫描。
#[tauri::command]
pub fn add_zip_source(
    name: String,
    zip_path: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Source, String> {
    let path = {
        let p = PathBuf::from(zip_path.trim());
        if let Ok(home) = dirs_home() {
            if let Ok(rel) = p.strip_prefix("~") {
                home.join(rel)
            } else {
                p
            }
        } else {
            p
        }
    };
    let display_name = if name.trim().is_empty() {
        path.file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "导入压缩包".into())
    } else {
        name
    };
    import_zip_file(display_name, &path, state.inner(), &app).map_err(|e| e.to_string())
}

/// 拖拽导入压缩包：前端读不到拖入文件的磁盘路径（dragDropEnabled=false 时
/// HTML5 drop 只有内容），所以把字节（base64）传过来，落一个临时 zip 后走
/// 同一条 import_zip_file 流程。解压是重 IO，丢到 blocking 线程池，
/// 不占 WebView 主线程。
#[tauri::command]
pub async fn add_zip_bytes(
    name: String,
    file_name: String,
    data_b64: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Source, String> {
    let state = state.inner().clone();
    let display_name = if name.trim().is_empty() {
        PathBuf::from(file_name.trim())
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "导入压缩包".into())
    } else {
        name
    };
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_b64.trim())
            .context("解码拖入文件失败")?;
        if bytes.is_empty() {
            bail!("拖入的文件是空的");
        }
        if bytes.len() as u64 > MAX_TOTAL_BYTES {
            bail!("压缩包超过 512MB 上限，已拒收");
        }
        let repos = crate::paths::repos_dir()?;
        std::fs::create_dir_all(&repos).with_context(|| format!("create {}", repos.display()))?;
        let tmp = repos.join(format!("drop-{}.zip", Uuid::new_v4()));
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        let result = import_zip_file(display_name, &tmp, &state, &app);
        let _ = std::fs::remove_file(&tmp);
        result
    })
    .await
    .map_err(|e| format!("后台导入任务失败: {e}"))?
    .map_err(|e| e.to_string())
}

fn dirs_home() -> Result<std::path::PathBuf, anyhow::Error> {
    dirs::home_dir().context("could not determine home directory")
}

/// 弹出系统文件选择框（只允许 zip / .skill）。
#[tauri::command]
pub async fn pick_archive_file(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .add_filter("Skill 压缩包", &["zip", "skill"])
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let result = rx.recv().map_err(|e| e.to_string())?;
    Ok(result.map(|p| p.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill_md(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: t\n---\nbody").unwrap();
    }

    #[test]
    fn single_skill_detected_after_collapse() {
        let tmp = std::env::temp_dir().join(format!("sm-zip-{}", Uuid::new_v4()));
        // macOS zip 的形态：根下只有一个技能目录（__MACOSX 解压时已剔除）。
        let root = tmp.join("extract");
        write_skill_md(&root.join("business-overview"));
        let collapsed = collapse_single_root(root);
        assert!(is_single_skill(&collapsed), "折叠后的根应直接含 SKILL.md");

        // 另一种形态：SKILL.md 就在压缩包顶层（无目录可折叠）。
        let flat = tmp.join("flat");
        write_skill_md(&flat);
        assert!(is_single_skill(&flat));

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn multi_skill_zip_is_not_single_skill() {
        let tmp = std::env::temp_dir().join(format!("sm-zip-{}", Uuid::new_v4()));
        let root = tmp.join("extract");
        for d in ["alpha", "beta"] {
            write_skill_md(&root.join(d));
        }
        let kept = collapse_single_root(root.clone());
        assert_eq!(kept, root, "多个子目录时不应折叠");
        assert!(!is_single_skill(&kept), "多技能集合不应进中央仓库");

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn vault_name_dedup_appends_suffix() {
        let tmp = std::env::temp_dir().join(format!("sm-zip-{}", Uuid::new_v4()));
        let vault = tmp.join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        assert_eq!(next_available_name(&vault, "s"), vault.join("s"));

        std::fs::create_dir_all(vault.join("s")).unwrap();
        assert_eq!(next_available_name(&vault, "s"), vault.join("s-2"));

        std::fs::create_dir_all(vault.join("s-2")).unwrap();
        assert_eq!(next_available_name(&vault, "s"), vault.join("s-3"));

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn single_skill_lands_in_vault_and_reuses_its_source() {
        let tmp = std::env::temp_dir().join(format!("sm-zip-e2e-{}", Uuid::new_v4()));
        let vault = tmp.join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        let mut persisted = crate::state::PersistedState::current();
        persisted.ui.vault_path = vault.to_string_lossy().to_string();
        let state = crate::state::AppState {
            data_dir: tmp.clone(),
            state_path: tmp.join("state.json"),
            inner: parking_lot::RwLock::new(persisted),
        };

        // 模拟解压产物：vault 之外一个含 SKILL.md 的技能目录。
        let staged = tmp.join("staged").join("demo-skill");
        write_skill_md(&staged);

        // ① 首次导入：注册「中央仓库」来源，技能挪进仓库。
        let src = install_single_skill_into_vault(&state, &staged, "demo-skill").unwrap();
        assert_eq!(src.name, "中央仓库");
        assert_eq!(Path::new(&src.location), vault);
        assert!(vault.join("demo-skill").join("SKILL.md").is_file());
        assert!(!staged.exists(), "技能应从解压暂存目录挪走");
        assert_eq!(state.inner.read().sources.len(), 1);

        // ② 再次导入同名技能：目录重名 -2，来源复用、不重复注册。
        let staged2 = tmp.join("staged2").join("demo-skill");
        write_skill_md(&staged2);
        let src2 = install_single_skill_into_vault(&state, &staged2, "demo-skill").unwrap();
        assert_eq!(src2.id, src.id, "应复用已注册的中央仓库来源");
        assert!(vault.join("demo-skill-2").join("SKILL.md").is_file());
        assert_eq!(state.inner.read().sources.len(), 1);

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
