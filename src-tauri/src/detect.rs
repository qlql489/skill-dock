//! Agent installation detection — ported from multica's `probeAgentCLIs`.
//!
//! An agent counts as "installed" if EITHER its CLI binary resolves on PATH
//! OR (for agents without a published CLI) its home config directory exists.
//! This is purely informational + used to auto-enable targets the user
//! hasn't manually touched; it never uninstalls or overrides a user choice.
//!
//! Detection is cheap (PATH lookups + a few stat calls) and runs on every
//! launch, so results are not cached across runs.

use crate::state::AgentTarget;
use std::path::Path;

/// The detection rule for a known agent: the CLI binary name to look up on
/// PATH (if any), and a home-relative directory to stat as a fallback (if
/// any). At least one of the two is `Some` for every agent we ship.
struct Rule {
    /// `exec.LookPath` / `which` target. None for agents that ship no CLI
    /// (e.g. MiniMax, IDE plugins) — those rely solely on directory presence.
    cli: Option<&'static str>,
    /// Home-relative path (e.g. `.openclaw`) to stat when the CLI is absent
    /// or wasn't found on PATH. None means "CLI-only, no dir fallback".
    fallback_dir: Option<&'static str>,
}

/// The canonical agent registry: target id → detection rule.
///
/// CLI names are ported verbatim from multica's `defaultAgentCommandNames`
/// (`server/internal/daemon/config.go`). Note the key≠cli mismatches:
/// kiro→kiro-cli, antigravity→agy, qoder→qodercli, trae→traecli.
fn rule_for(id: &str) -> Option<Rule> {
    let rule = match id {
        "claude-code" => Rule { cli: Some("claude"), fallback_dir: Some(".claude") },
        "codex" => Rule { cli: Some("codex"), fallback_dir: None }, // .app bundle handled separately
        "zcode" => Rule { cli: Some("zcode"), fallback_dir: Some(".zcode") },
        "deepseek-harness" | "deepseek_harness" => Rule { cli: None, fallback_dir: Some(".dsh") },
        "minimax" => Rule { cli: None, fallback_dir: Some(".minimax") },
        "minimax-builtin" => Rule { cli: None, fallback_dir: Some(".minimax") },
        "trae" => Rule { cli: Some("traecli"), fallback_dir: Some(".trae") },
        "codebuddy" => Rule { cli: Some("codebuddy"), fallback_dir: Some(".codebuddy") },
        "kiro" => Rule { cli: Some("kiro-cli"), fallback_dir: Some(".kiro") },
        "qwen" => Rule { cli: Some("qwen"), fallback_dir: Some(".qwen") },
        "qoder" => Rule { cli: Some("qodercli"), fallback_dir: Some(".qoder") },
        "qoderwork" => Rule { cli: Some("qodercli"), fallback_dir: Some(".qoderwork") },
        "hanako" => Rule { cli: None, fallback_dir: Some(".hanako") },
        "lingma" => Rule { cli: None, fallback_dir: Some(".lingma") },
        "iflow" => Rule { cli: None, fallback_dir: Some(".iflow") },
        "kode" => Rule { cli: None, fallback_dir: Some(".kode") },
        "continue" => Rule { cli: None, fallback_dir: Some(".continue") },
        "openclaw" => Rule { cli: Some("openclaw"), fallback_dir: Some(".openclaw") },
        "workbuddy" => Rule { cli: None, fallback_dir: Some(".workbuddy") },
        "hermes" => Rule { cli: Some("hermes"), fallback_dir: Some(".hermes") },
        "antigravity" => Rule { cli: Some("agy"), fallback_dir: Some(".antigravity") },
        "kimi-code" => Rule { cli: Some("kimi"), fallback_dir: Some(".kimi") },
        // ---- 第二批目录扩充的检测规则（路径对标 skills-manager 适配器表）----
        "cursor" => Rule { cli: None, fallback_dir: Some(".cursor") },
        "windsurf" => Rule { cli: None, fallback_dir: Some(".codeium/windsurf") },
        "github-copilot" => Rule { cli: Some("copilot"), fallback_dir: Some(".copilot") },
        "gemini-cli" => Rule { cli: Some("gemini"), fallback_dir: Some(".gemini") },
        "opencode" => Rule { cli: Some("opencode"), fallback_dir: Some(".config/opencode") },
        "goose" => Rule { cli: Some("goose"), fallback_dir: Some(".config/goose") },
        "roo-code" => Rule { cli: None, fallback_dir: Some(".roo") },
        "kilo-code" => Rule { cli: None, fallback_dir: Some(".kilocode") },
        "droid" => Rule { cli: None, fallback_dir: Some(".factory") },
        "crush" => Rule { cli: None, fallback_dir: Some(".config/crush") },
        "pi" => Rule { cli: Some("pi"), fallback_dir: Some(".pi/agent") },
        "neovate" => Rule { cli: None, fallback_dir: Some(".neovate") },
        "openhands" => Rule { cli: None, fallback_dir: Some(".openhands") },
        "grok" => Rule { cli: Some("grok"), fallback_dir: Some(".grok") },
        "augment" => Rule { cli: None, fallback_dir: Some(".augment") },
        "junie" => Rule { cli: None, fallback_dir: Some(".junie") },
        "deepagents" => Rule { cli: None, fallback_dir: Some(".deepagents") },
        _ => return None, // user-defined custom target — unknown, not detected
    };
    Some(rule)
}

/// Candidate macOS app-bundle paths for the bundled Codex CLI. OpenAI moved
/// the desktop app from `Codex.app` to `ChatGPT.app`; we check both, system
/// `/Applications` before user `~/Applications`. Ported from multica's
/// `codexDesktopAppBundlePaths`.
fn codex_desktop_paths(home: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = vec![
        Path::new("/Applications/ChatGPT.app/Contents/Resources/codex").to_path_buf(),
        Path::new("/Applications/Codex.app/Contents/Resources/codex").to_path_buf(),
    ];
    paths.push(home.join("Applications/ChatGPT.app/Contents/Resources/codex"));
    paths.push(home.join("Applications/Codex.app/Contents/Resources/codex"));
    paths
}

/// Is the given agent installed on this machine?
///
/// Priority: (1) CLI binary on PATH via `which`, (2) Codex Desktop .app
/// bundle stat (codex only), (3) home config dir exists. Any hit ⇒ true.
pub fn is_agent_installed(id: &str, home: &Path) -> bool {
    let Some(rule) = rule_for(id) else {
        return false; // unknown / custom target
    };

    // (1) CLI on PATH.
    if let Some(cli) = rule.cli {
        if which::which(cli).is_ok() {
            return true;
        }
    }

    // (2) Codex Desktop app-bundle fallback (the only .app special case,
    // matching multica).
    if id == "codex" {
        for p in codex_desktop_paths(home) {
            if p.exists() {
                return true;
            }
        }
    }

    // (3) Home config dir fallback.
    if let Some(dir) = rule.fallback_dir {
        if home.join(dir).exists() {
            return true;
        }
    }

    false
}

/// Probe every target and write the result into its `detected` field.
/// Returns the count of newly-detected targets (for logging / toast).
pub fn detect_all(targets: &mut [AgentTarget]) -> usize {
    let Some(home) = dirs::home_dir() else {
        log::warn!("detect: could not determine home dir, skipping");
        return 0;
    };
    let mut detected_count = 0;
    for t in targets.iter_mut() {
        let installed = match rule_for(&t.id) {
            Some(_) => is_agent_installed(&t.id, &home),
            None => {
                // 自定义目标没有内置规则：skills 目录本身或其父目录存在
                // 即视为已安装（用户指向 ~/.pi/agent/skills 时 ~/.pi 存在
                // 就说明 agent 装过，哪怕 skills 目录还没生成）。
                t.skills_dir.exists()
                    || t.skills_dir.parent().map(|p| p.exists()).unwrap_or(false)
            }
        };
        if installed {
            detected_count += 1;
        }
        t.detected = installed;
    }
    log::info!("detect: {detected_count}/{} targets detected as installed", targets.len());
    detected_count
}

/// Auto-enable pass: for any target that is `detected` but not yet `enabled`
/// AND the user has never manually toggled it (`!user_touched`), flip
/// `enabled` to true. Returns the count of newly-enabled targets.
///
/// This is the "respect user choice" rule: a target the user deliberately
/// disabled (user_touched=true) is left alone even if the agent is installed.
pub fn auto_enable_untouched(targets: &mut [AgentTarget]) -> usize {
    let mut enabled_count = 0;
    for t in targets.iter_mut() {
        if t.detected && !t.enabled && !t.user_touched {
            t.enabled = true;
            enabled_count += 1;
            log::info!("detect: auto-enabled untouched target '{}'", t.id);
        }
    }
    if enabled_count > 0 {
        log::info!("detect: auto-enabled {enabled_count} newly-detected target(s)");
    }
    enabled_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn deepseek_harness_is_detected_from_home_directory() {
        let home = unique_test_dir();
        assert!(!is_agent_installed("deepseek-harness", &home));

        fs::create_dir_all(home.join(".dsh")).unwrap();
        assert!(is_agent_installed("deepseek-harness", &home));
        // Keep compatibility with the older underscore-style key used by
        // some external agent registries.
        assert!(is_agent_installed("deepseek_harness", &home));

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn zcode_is_detected_via_home_directory_fallback() {
        // 不能断言空目录时返回 false：本机 PATH 上若装了 zcode CLI 会直接
        // 命中第一优先级。这里只验证规则已注册且目录兜底路径正确。
        let home = unique_test_dir();
        fs::create_dir_all(home.join(".zcode")).unwrap();
        assert!(is_agent_installed("zcode", &home));
        fs::remove_dir_all(home).unwrap();
    }

    fn unique_test_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "skill-dock-detect-{nanos}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
