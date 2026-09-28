//! Skill 市场集成 —— 多市场源:skills.sh / 腾讯 SkillHub / ClawHub。
//!
//! 三个源的形态(全部实测过,无需 API key):
//!   - skills.sh:搜索走官方 `/api/search`;榜单解析页面 RSC payload 内嵌的
//!     `initialSkills` 转义 JSON。安装 = 浅克隆条目对应的 GitHub 仓库。
//!   - 腾讯 SkillHub(api.skillhub.tencent.com):列表/搜索/排序一个接口,
//!     `GET /api/skills?page=&pageSize=&keyword=&sortBy=`;下载
//!     `GET /api/v1/download?slug=` 302 到腾讯 COS 的版本化 zip。
//!   - ClawHub(clawhub.ai,OpenClaw 生态):搜索 `GET /api/search?q=&limit=`,
//!     榜单 `GET /api/v1/trending`;下载 `GET /api/v1/download?slug=&ownerHandle=`
//!     直接返回 zip。
//!
//! HTTP 统一走系统 `curl`(与 git 同一哲学:用系统工具,免新增依赖,
//! 且自动继承用户环境的代理/证书配置)。
//!
//! 安装分两条路:
//!   - skillssh:克隆 GitHub 仓库 → find_skill_dir → 复制进 market 目录。
//!   - skillhub / clawhub:curl -L 下载 zip → archive::extract_archive 安全解压
//!     (zip-slip 防护 + symlink 不落地)→ 定位 skill 目录 → 复制。
//!
//! 所有已装技能记在 `market/.market-registry.json`(含 provider),用于冲突
//! 判定:同源重装 = 覆盖更新,异源同名 = 自动 `-N` 后缀并存,绝不静默覆盖
//! 来历不明的同名目录。首次安装会把 market 目录注册为常驻本地来源
//! 「Skill 市场」,享受 watcher 自动重扫,之后照常软链安装到各 agent。

use crate::paths::{data_dir, repos_dir};
use crate::state::{PersistedState, Source, SourceKind, SourceMode};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const PROVIDER_SKILLSSH: &str = "skillssh";
pub const PROVIDER_CLAWHUB: &str = "clawhub";
pub const PROVIDER_SKILLHUB: &str = "skillhub";

const MARKET_BASE: &str = "https://www.skills.sh";
const CLAWHUB_BASE: &str = "https://clawhub.ai";
const SKILLHUB_API: &str = "https://api.skillhub.tencent.com";
const HTTP_TIMEOUT_SECS: &str = "20";
/// zip 下载(可能几十 MB)给更宽的超时。
const DOWNLOAD_TIMEOUT_SECS: &str = "120";
/// 页面 UA:skills.sh 是 Next.js 站点,裸 curl UA 也能拿到数据,
/// 但带浏览器 UA 最稳。
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 skill-dock/1.0";
/// 榜单缓存 TTL —— 站点数据本身就是低频更新,5 分钟足够新鲜。
const LEADERBOARD_TTL: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------

/// 市场条目 —— 三个 provider 统一成一张脸。字段对齐各源的 JSON(camelCase)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketEntry {
    /// skillssh | clawhub | skillhub
    pub provider: String,
    /// 全局唯一键 "{provider}:{source}/{skill_id}",前端用它标记"已安装"。
    pub id: String,
    /// slug / 目录名(安装时定位用)。
    pub skill_id: String,
    pub name: String,
    /// skills.sh / clawhub:GitHub "owner/repo" 或作者 handle;skillhub:空。
    #[serde(default)]
    pub source: String,
    /// 优先安装量,没有则退回下载量。
    #[serde(default)]
    pub installs: u64,
    /// 一句话简介(skillssh 没有此数据,为空串)。
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_official: bool,
}

fn entry_id(provider: &str, source: &str, skill_id: &str) -> String {
    format!("{provider}:{source}/{skill_id}")
}

fn make_entry(provider: &str, skill_id: &str, name: &str, source: &str, installs: u64, description: &str, is_official: bool) -> MarketEntry {
    MarketEntry {
        provider: provider.to_string(),
        id: entry_id(provider, source, skill_id),
        skill_id: skill_id.to_string(),
        name: name.to_string(),
        source: source.to_string(),
        installs,
        description: description.to_string(),
        is_official,
    }
}

/// 已装市场技能的登记记录(落盘在 market/.market-registry.json)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketRecord {
    /// skillssh | clawhub | skillhub。旧记录缺省视为 skillssh。
    #[serde(default = "default_provider")]
    pub provider: String,
    /// clawhub/skillhub 作者 handle;skills.sh 为 GitHub "owner/repo";
    /// skillhub 无作者概念,为空串。
    pub source: String,
    pub skill_id: String,
    /// market 根目录下的目录名。
    pub dir: String,
    pub commit: Option<String>,
    pub installed_at: String,
}

fn default_provider() -> String {
    PROVIDER_SKILLSSH.to_string()
}

/// 安装结果(返回给前端)。
#[derive(Debug, Clone, Serialize)]
pub struct InstallOutcome {
    pub source_id: String,
    pub skill_id: String,
    pub dir_name: String,
    pub name: String,
    pub replaced: bool,
    pub files: u64,
}

// ---------------------------------------------------------------------------
// 各源原始 payload 结构(只取需要的字段,其余忽略)
// ---------------------------------------------------------------------------

/// ClawHub 搜索/榜单条目(两处结构基本一致,榜单的 author 信息可能在
/// install.reference 或 native 里,统一在这里兜底)。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClawhubItem {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    official: bool,
    #[serde(default)]
    owner_handle: String,
    #[serde(default)]
    install: Option<ClawhubInstallRef>,
    #[serde(default)]
    native: Option<serde_json::Value>,
    #[serde(default)]
    metrics: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ClawhubInstallRef {
    #[serde(default)]
    reference: String,
}

impl ClawhubItem {
    fn owner_and_slug(&self) -> Option<(String, String)> {
        if !self.owner_handle.is_empty() && !self.slug.is_empty() {
            return Some((self.owner_handle.clone(), self.slug.clone()));
        }
        let reference = self.install.as_ref()?.reference.clone();
        let mut parts = reference.split('/');
        let owner = parts.next()?.to_string();
        let slug = parts.next()?.to_string();
        if owner.is_empty() || slug.is_empty() {
            None
        } else {
            Some((owner, slug))
        }
    }

    fn summary_text(&self) -> String {
        if !self.summary.is_empty() {
            return self.summary.clone();
        }
        // 榜单条目的 summary 藏在 native.skill.summary
        self.native
            .as_ref()
            .and_then(|n| n.pointer("/skill/summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    fn installs(&self) -> u64 {
        if self.downloads > 0 {
            return self.downloads;
        }
        self.metrics
            .as_ref()
            .and_then(|m| m.get("lifetimeInstalls"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    }
}

#[derive(Debug, Deserialize)]
struct ClawhubSearchResp {
    #[serde(default)]
    results: Vec<ClawhubItem>,
}

#[derive(Debug, Deserialize)]
struct ClawhubTrendingResp {
    #[serde(default)]
    items: Vec<ClawhubItem>,
}

/// 腾讯 SkillHub:`{code, message, data:{skills:[...], total}}`。
#[derive(Debug, Deserialize)]
struct SkillhubResp {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<SkillhubData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillhubData {
    #[serde(default)]
    skills: Vec<SkillhubSkill>,
    /// 保留给将来做分页;当前前端按页拉取,不消费它。
    #[allow(dead_code)]
    #[serde(default)]
    total: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillhubSkill {
    #[serde(default)]
    slug: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    installs: u64,
}

#[derive(Debug, Deserialize)]
struct SearchResp {
    #[serde(default)]
    skills: Vec<MarketEntry>,
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// 极简 percent-encode(查询串够用,不引依赖)。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 搜索 / 榜单(按 provider 分发)
// ---------------------------------------------------------------------------

fn http_get(url: &str) -> Result<String> {
    let out = http_get_full(url, HTTP_TIMEOUT_SECS)?;
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// 下载二进制(跟随重定向 —— SkillHub 的下载是 302 到 COS)。
fn http_download(url: &str, dest: &Path) -> Result<u64> {
    if which::which("curl").is_err() {
        bail!("系统没有 curl,无法访问 skill 市场");
    }
    let out = Command::new("curl")
        .args([
            "-sSf",
            "--location",
            "--max-time",
            DOWNLOAD_TIMEOUT_SECS,
            "-A",
            BROWSER_UA,
            "-o",
            &dest.to_string_lossy(),
            url,
        ])
        .output()
        .context("启动 curl 下载失败")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(dest);
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("下载失败: {err}");
    }
    let size = dest.metadata().map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

fn http_get_full(url: &str, timeout: &str) -> Result<Vec<u8>> {
    if which::which("curl").is_err() {
        bail!("系统没有 curl,无法访问 skill 市场");
    }
    let out = Command::new("curl")
        .args([
            "-sSf",
            "--location",
            "--max-time",
            timeout,
            "-A",
            BROWSER_UA,
            url,
        ])
        .output()
        .context("启动 curl 失败")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("请求市场数据失败: {err}");
    }
    Ok(out.stdout)
}

/// 搜索市场技能。query 为空时返回错误(前端空态走榜单)。
pub fn search(provider: &str, query: &str, limit: usize) -> Result<Vec<MarketEntry>> {
    match provider {
        PROVIDER_CLAWHUB => clawhub_search(query, limit),
        PROVIDER_SKILLHUB => skillhub_search(query, limit),
        _ => skillssh_search(query, limit),
    }
}

/// 榜单。board 仅对 skillssh 有区分(clawhub 走 trending,skillhub 按下载量)。
/// 榜单缓存：按 (provider, board) 分槽，互不挤占；TTL 5 分钟。
static LEADERBOARD_CACHE: Lazy<Mutex<HashMap<(String, String), (Instant, Vec<MarketEntry>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn cached_leaderboard(provider: &str, board: &str) -> Option<Vec<MarketEntry>> {
    let guard = LEADERBOARD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match guard.get(&(provider.to_string(), board.to_string())) {
        Some((at, entries)) if at.elapsed() < LEADERBOARD_TTL => Some(entries.clone()),
        _ => None,
    }
}

fn store_leaderboard(provider: &str, board: &str, entries: Vec<MarketEntry>) {
    LEADERBOARD_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((provider.to_string(), board.to_string()), (Instant::now(), entries));
}

/// 榜单。board 仅对 skillssh 有区分(clawhub 走 trending,skillhub 按下载量)。
pub fn leaderboard(provider: &str, board: &str) -> Result<Vec<MarketEntry>> {
    if let Some(hit) = cached_leaderboard(provider, board) {
        return Ok(hit);
    }
    let entries = match provider {
        PROVIDER_CLAWHUB => clawhub_trending()?,
        PROVIDER_SKILLHUB => skillhub_list()?,
        _ => skillssh_leaderboard(board)?,
    };
    store_leaderboard(provider, board, entries.clone());
    Ok(entries)
}

// ---- skills.sh ----

/// 搜索市场技能。query 为空时返回错误(前端空态走榜单)。
fn skillssh_search(query: &str, limit: usize) -> Result<Vec<MarketEntry>> {
    let q = query.trim();
    if q.is_empty() {
        bail!("搜索词为空");
    }
    let limit = limit.clamp(1, 100);
    let url = format!("{MARKET_BASE}/api/search?q={}&limit={}", urlencode(q), limit);
    let body = http_get(&url)?;
    let resp: SearchResp = serde_json::from_str(&body).context("解析搜索结果失败")?;
    Ok(resp
        .skills
        .into_iter()
        .map(|mut e| {
            e.provider = PROVIDER_SKILLSSH.to_string();
            e.id = entry_id(PROVIDER_SKILLSSH, &e.source, &e.skill_id);
            e
        })
        .collect())
}

fn board_path(board: &str) -> &'static str {
    match board {
        "trending" => "/trending",
        "hot" => "/hot",
        _ => "/",
    }
}

/// skills.sh 榜单(带进程内 TTL 缓存)。
fn skillssh_leaderboard(board: &str) -> Result<Vec<MarketEntry>> {
    // 只负责抓取与解析；缓存由 leaderboard() 的统一入口管理。
    let html = http_get(&format!("{MARKET_BASE}{}", board_path(board)))?;
    parse_initial_skills(&html)
}

// ---- ClawHub ----

fn clawhub_search(query: &str, limit: usize) -> Result<Vec<MarketEntry>> {
    let q = query.trim();
    if q.is_empty() {
        bail!("搜索词为空");
    }
    let limit = limit.clamp(1, 100);
    let url = format!("{CLAWHUB_BASE}/api/search?q={}&limit={}", urlencode(q), limit);
    let body = http_get(&url)?;
    let resp: ClawhubSearchResp = serde_json::from_str(&body).context("解析 ClawHub 搜索结果失败")?;
    Ok(resp
        .results
        .into_iter()
        .filter_map(|item| {
            let (owner, slug) = item.owner_and_slug()?;
            Some(make_entry(
                PROVIDER_CLAWHUB,
                &slug,
                if item.display_name.is_empty() { &slug } else { &item.display_name },
                &owner,
                item.installs(),
                &item.summary_text(),
                item.official,
            ))
        })
        .collect())
}

fn clawhub_trending() -> Result<Vec<MarketEntry>> {
    let body = http_get(&format!("{CLAWHUB_BASE}/api/v1/trending"))?;
    let resp: ClawhubTrendingResp = serde_json::from_str(&body).context("解析 ClawHub 榜单失败")?;
    Ok(resp
        .items
        .into_iter()
        .filter_map(|item| {
            let (owner, slug) = item.owner_and_slug()?;
            Some(make_entry(
                PROVIDER_CLAWHUB,
                &slug,
                if item.display_name.is_empty() { &slug } else { &item.display_name },
                &owner,
                item.installs(),
                &item.summary_text(),
                item.official,
            ))
        })
        .collect())
}

// ---- 腾讯 SkillHub ----

fn skillhub_fetch(params: &str) -> Result<Vec<MarketEntry>> {
    let url = format!("{SKILLHUB_API}/api/skills?{params}");
    let body = http_get(&url)?;
    let resp: SkillhubResp = serde_json::from_str(&body).context("解析 SkillHub 响应失败")?;
    if resp.code != 0 {
        bail!("SkillHub 返回错误: {} ({})", resp.message, resp.code);
    }
    Ok(resp
        .data
        .map(|d| d.skills)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.slug.is_empty())
        .map(|s| {
            make_entry(
                PROVIDER_SKILLHUB,
                &s.slug,
                if s.name.is_empty() { &s.slug } else { &s.name },
                "",
                if s.installs > 0 { s.installs } else { s.downloads },
                &s.description,
                false,
            )
        })
        .collect())
}

fn skillhub_list() -> Result<Vec<MarketEntry>> {
    // 按下载量降序 = 热度榜;默认排序是混杂的,不能直接用。
    skillhub_fetch("page=1&pageSize=50&sortBy=downloads&order=desc")
}

fn skillhub_search(query: &str, limit: usize) -> Result<Vec<MarketEntry>> {
    let q = query.trim();
    if q.is_empty() {
        bail!("搜索词为空");
    }
    let limit = limit.clamp(1, 60);
    skillhub_fetch(&format!(
        "page=1&pageSize={limit}&keyword={}",
        urlencode(q)
    ))
}

/// skills.sh 榜单条目的原始形态(camelCase),解析后再统一成 MarketEntry。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsshRaw {
    #[serde(default)]
    skill_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    installs: u64,
    #[serde(default)]
    is_official: bool,
}

/// 从页面 HTML 中切出 `initialSkills` 转义 JSON 数组并解析。
pub fn parse_initial_skills(html: &str) -> Result<Vec<MarketEntry>> {
    const MARKER: &str = "initialSkills\\\":";
    let pos = html
        .find(MARKER)
        .with_context(|| "skills.sh 页面里没有找到榜单数据(站点结构可能已变)")?;
    let rest = &html[pos + MARKER.len()..];
    let start = rest.find('[').context("榜单标记后没有数组")?;
    let json_text = scan_js_escaped_array(&rest[start..])?;
    let raw: Vec<SkillsshRaw> =
        serde_json::from_str(&json_text).map_err(|e| anyhow::anyhow!("解析榜单 JSON 失败: {e}"))?;
    Ok(raw
        .into_iter()
        .filter(|r| !r.skill_id.is_empty())
        .map(|r| {
            make_entry(
                PROVIDER_SKILLSSH,
                &r.skill_id,
                if r.name.is_empty() { &r.skill_id } else { &r.name },
                &r.source,
                r.installs,
                "",
                r.is_official,
            )
        })
        .collect())
}

/// 从 RSC payload 切出数组:payload 是被 JS 字符串转义过的 JSON,引号以
/// `\"` 形态出现。对整段流做反转义,并在**还原后的流**上做字符串感知的
/// 括号平衡——否则字符串里的 `]`(如技能名 "x]y")会提前截断。
fn scan_js_escaped_array(s: &str) -> Result<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() / 2);
    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        // 无条件反转义:payload 里 JSON 的引号本身就是转义形态,
        // 不存在"只在字符串内处理转义"的前提。
        let c = if c == '\\' {
            let e = *chars.get(i).context("payload 在转义符处截断")?;
            i += 1;
            match e {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'u' => {
                    let hex: String = chars.get(i..i + 4).context("bad \\u")?.iter().collect();
                    let cp =
                        u32::from_str_radix(&hex, 16).map_err(|_| anyhow::anyhow!("bad \\u"))?;
                    i += 4;
                    char::from_u32(cp).unwrap_or('\u{fffd}')
                }
                other => other,
            }
        } else {
            c
        };
        out.push(c);
        // 结构状态跟踪在还原后的字符上进行。
        if in_str {
            if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                _ => {}
            }
        }
    }
    bail!("payload 括号不平衡");
}

// ---------------------------------------------------------------------------
// 安装
// ---------------------------------------------------------------------------

pub fn market_root() -> Result<PathBuf> {
    Ok(data_dir()?.join("market"))
}

fn registry_path() -> Result<PathBuf> {
    Ok(market_root()?.join(".market-registry.json"))
}

/// 读登记表;文件不存在(从未装过)返回空表。
pub fn read_registry() -> Result<BTreeMap<String, MarketRecord>> {
    let path = registry_path()?;
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn write_registry(reg: &BTreeMap<String, MarketRecord>) -> Result<()> {
    let path = registry_path()?;
    let bytes = serde_json::to_vec_pretty(reg)?;
    std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
}

/// 已装登记表(命令层直接暴露给前端做"已安装"标记)。
pub fn registry_list() -> Vec<MarketRecord> {
    read_registry().unwrap_or_default().into_values().collect()
}

/// 校验单段来源标识(clawhub 作者 handle):不允许路径分隔符。
fn validate_source_part(s: &str, what: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 100
        || s.contains('/')
        || s.contains('\\')
        || s.contains("..")
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        bail!("非法的{what}: {s}");
    }
    Ok(())
}

/// 校验 "owner/repo" 形态,防路径注入。
fn validate_owner_repo(s: &str) -> Result<()> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        bail!("来源必须是 owner/repo 形式: {s}");
    }
    for p in &parts {
        if p.is_empty()
            || p.len() > 100
            || *p == "."
            || *p == ".."
            || p.starts_with('.')
            || !p
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            bail!("来源包含非法字符: {s}");
        }
    }
    Ok(())
}

/// 校验 skill_id:只作为目录名使用,不允许路径分隔符。
fn validate_skill_id(s: &str) -> Result<()> {
    if s.is_empty()
        || s.len() > 100
        || s.contains('/')
        || s.contains('\\')
        || s.contains("..")
        || s.contains('\0')
    {
        bail!("非法的 skill 标识: {s}");
    }
    Ok(())
}

/// 目录名清洗:保留字母数字与 - _ .,其余折叠成 '-',空则兜底 "skill"。
fn sanitize_dir_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    let mut out = if out.is_empty() { "skill".to_string() } else { out };
    out.truncate(64);
    out
}

/// 安装入口:按 provider 分发。slow IO 全程无锁,调用方之后用短写锁补
/// 来源注册/隐藏清理。
pub fn install_skill(provider: &str, source: &str, skill_id: &str) -> Result<InstallOutcome> {
    match provider {
        PROVIDER_CLAWHUB => {
            validate_source_part(source, "作者 handle")?;
            validate_skill_id(skill_id)?;
            let url = format!("{CLAWHUB_BASE}/api/v1/download?slug={skill_id}&ownerHandle={source}");
            install_zip_skill(PROVIDER_CLAWHUB, source, skill_id, &url)
        }
        PROVIDER_SKILLHUB => {
            validate_skill_id(skill_id)?;
            let url = format!("{SKILLHUB_API}/api/v1/download?slug={skill_id}");
            install_zip_skill(PROVIDER_SKILLHUB, "", skill_id, &url)
        }
        _ => download_skill_from_github(source, skill_id),
    }
}

/// 慢路径(无锁):克隆仓库 → 定位 skill → 复制进 market 根目录并登记。
fn download_skill_from_github(owner_repo: &str, skill_id: &str) -> Result<InstallOutcome> {
    validate_owner_repo(owner_repo)?;
    validate_skill_id(skill_id)?;

    let tmp = prepare_repos_tmp()?;
    // clone_repo 失败时自带清理;成功后这里保证临时仓库被删。
    let result = (|| {
        let url = format!("https://github.com/{owner_repo}.git");
        crate::sources::clone_repo(&url, &tmp, None).with_context(|| format!("克隆 {url} 失败"))?;
        let commit = crate::sources::head_commit(&tmp).ok();

        let src_dir = find_skill_dir(&tmp, skill_id)?;
        install_from_dir(&src_dir, PROVIDER_SKILLSSH, owner_repo, skill_id, commit)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// zip 下载安装路径(SkillHub / ClawHub):curl -L 下载 → 安全解压 → 定位 →
/// 落库。SkillHub 的下载是 302 到 COS,curl --location 跟随。
fn install_zip_skill(provider: &str, source: &str, skill_id: &str, url: &str) -> Result<InstallOutcome> {
    let repos = repos_dir()?;
    std::fs::create_dir_all(&repos)?;
    let zip_path = repos.join(format!("market-{}.zip", Uuid::new_v4()));
    let size = http_download(url, &zip_path)?;
    if size == 0 {
        let _ = std::fs::remove_file(&zip_path);
        bail!("下载到的 zip 是空的: {url}");
    }

    let result = (|| {
        let outcome = crate::archive::extract_archive(&zip_path)
            .context("zip 解压失败(已做 zip-slip 与符号链接防护)")?;
        // 解压根即 skill 根(ClawHub/SkillHub 的 zip SKILL.md 都在根);
        // 若布局不同,退回 find_skill_dir。
        let dest = outcome.dest_dir.clone();
        let skill_dir = if dest.join("SKILL.md").is_file() {
            dest.clone()
        } else {
            find_skill_dir(&dest, skill_id)?
        };
        let installed = install_from_dir(
            &skill_dir,
            provider,
            source,
            skill_id,
            None,
        );
        // 清理解压目录(dest 可能被 collapse_single_root 降了一层,从 uuid 层删)。
        let _ = std::fs::remove_dir_all(cleanup_extract_root(&dest));
        installed
    })();
    let _ = std::fs::remove_file(&zip_path);
    result
}

/// 解压落在 repos/zips/<uuid>[/<sub>],清理时回到 <uuid> 层删干净。
fn cleanup_extract_root(dest: &Path) -> PathBuf {
    if dest
        .file_name()
        .map(|n| n == "zips")
        .unwrap_or(false)
    {
        return dest.to_path_buf();
    }
    match dest.parent() {
        Some(p) if p.file_name().map(|n| n == "zips").unwrap_or(false) => p.to_path_buf(),
        Some(p) => p.to_path_buf(),
        None => dest.to_path_buf(),
    }
}

/// 准备一个用于 git 克隆的临时目录(market-<uuid>)。
fn prepare_repos_tmp() -> Result<PathBuf> {
    let repos = repos_dir()?;
    // 临时克隆目录的父目录必须存在,否则 git 以 ENOENT 失败
    //(正常启动时 lib.rs 已建好,这里兜底)。
    std::fs::create_dir_all(&repos)?;
    Ok(repos.join(format!("market-{}", Uuid::new_v4())))
}

/// 公共落库尾巴:市场根目录下的命名/冲突判定 → 复制 → 登记。
/// 返回的 outcome.source_id 为空,由命令层注册来源后回填。
fn install_from_dir(
    src_dir: &Path,
    provider: &str,
    source: &str,
    skill_id: &str,
    commit: Option<String>,
) -> Result<InstallOutcome> {
    let root = market_root()?;
    std::fs::create_dir_all(&root)?;

    let fm_name = frontmatter_name(&src_dir.join("SKILL.md")).unwrap_or_default();
    let skill_name = if fm_name.is_empty() {
        skill_id.to_string()
    } else {
        fm_name
    };

    let registry = read_registry().unwrap_or_default();
    let base = sanitize_dir_name(skill_id);
    let mut dir_name = base.clone();
    let mut n = 2u32;
    let mut replaced = false;
    let dest = loop {
        let dest = root.join(&dir_name);
        if !dest.exists() {
            break dest;
        }
        // 同一来源的同一技能 = 重装/更新,覆盖;其余一律换名并存,
        // 绝不覆盖来历不明的目录(与软链所有权保护同一原则)。
        let same_origin = registry
            .get(&dir_name)
            .map(|r| {
                let rp = if r.provider.is_empty() {
                    PROVIDER_SKILLSSH
                } else {
                    r.provider.as_str()
                };
                rp == provider && r.source == source && r.skill_id == skill_id
            })
            .unwrap_or(false);
        if same_origin {
            std::fs::remove_dir_all(&dest)
                .with_context(|| format!("清理旧版本 {}", dest.display()))?;
            replaced = true;
            break dest;
        }
        if n > 99 {
            bail!("同名 skill 过多,放弃命名: {base}");
        }
        dir_name = format!("{base}-{n}");
        n += 1;
    };

    let files = copy_skill_dir(&src_dir, &dest)?;
    if files == 0 {
        bail!("skill 目录是空的: {}", src_dir.display());
    }

    let mut registry = registry;
    registry.insert(
        dir_name.clone(),
        MarketRecord {
            provider: provider.to_string(),
            source: source.to_string(),
            skill_id: skill_id.to_string(),
            dir: dir_name.clone(),
            commit,
            installed_at: Utc::now().to_rfc3339(),
        },
    );
    write_registry(&registry)?;

    Ok(InstallOutcome {
        source_id: String::new(), // 命令层注册来源后回填
        skill_id: skill_id.to_string(),
        dir_name,
        name: skill_name,
        replaced,
        files,
    })
}

/// 在 market 根目录下找到(或创建)常驻本地来源「Skill 市场」。
/// 返回 (来源, 是否新建)。调用方持有写锁。
pub fn ensure_market_source(store: &mut PersistedState) -> Result<(Source, bool)> {
    let location = market_root()?.display().to_string();
    if let Some(s) = store
        .sources
        .iter()
        .find(|s| s.kind == SourceKind::Local && s.location == location)
    {
        return Ok((s.clone(), false));
    }
    let src = crate::sources::add_local(
        store,
        "Skill 市场".to_string(),
        location,
        SourceMode::Flat,
        Vec::new(),
    )?;
    Ok((src, true))
}

/// 在克隆出的仓库里定位 skill 目录:先试常见布局,再递归兜底
/// (目录名匹配 → SKILL.md frontmatter 的 name 匹配)。
pub fn find_skill_dir(repo: &Path, skill_id: &str) -> Result<PathBuf> {
    const LAYOUTS: &[&str] = &["", "skills", ".agents/skills", ".claude/skills"];
    for layout in LAYOUTS {
        let mut cand = repo.to_path_buf();
        for seg in layout.split('/').filter(|s| !s.is_empty()) {
            cand.push(seg);
        }
        cand.push(skill_id);
        if cand.join("SKILL.md").is_file() {
            return Ok(cand);
        }
    }

    let mut by_dir_name: Option<PathBuf> = None;
    let mut by_frontmatter: Option<PathBuf> = None;
    for entry in WalkDir::new(repo)
        .max_depth(6)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(name.starts_with('.') || crate::scanner::should_skip_dir(&name))
        })
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        if entry.file_name().to_string_lossy() == skill_id {
            if path.join("SKILL.md").is_file() {
                return Ok(path.to_path_buf());
            }
            if by_dir_name.is_none() {
                by_dir_name = Some(path.to_path_buf());
            }
        }
        if by_frontmatter.is_none() {
            let fm = path.join("SKILL.md");
            if fm.is_file() && frontmatter_name(&fm).as_deref() == Some(skill_id) {
                by_frontmatter = Some(path.to_path_buf());
            }
        }
    }
    by_dir_name
        .or(by_frontmatter)
        .with_context(|| format!("仓库里找不到 skill `{skill_id}`(常见布局与递归均未命中)"))
}

use walkdir::WalkDir;

/// 读取 SKILL.md frontmatter 的 name(够用即可,不引 YAML 库)。
fn frontmatter_name(skill_md: &Path) -> Option<String> {
    let content = std::fs::read_to_string(skill_md).ok()?;
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let v = rest.trim().trim_matches(['"', '\'']);
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 递归复制 skill 目录。跳过所有点文件(.git / .DS_Store / 缓存)和一切
/// 符号链接(防 exfiltration,与压缩包导入同一防线)。返回复制的文件数。
fn copy_skill_dir(src: &Path, dst: &Path) -> Result<u64> {
    std::fs::create_dir_all(dst)?;
    let mut count = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            count += copy_skill_dir(&entry.path(), &dst.join(&name))?;
        } else {
            std::fs::copy(entry.path(), dst.join(&name))?;
            count += 1;
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

use crate::state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// 搜索市场技能(provider = skillssh | clawhub | skillhub;空词由前端控制
/// 走榜单,这里兜底报错)。
#[tauri::command]
pub fn market_search(
    provider: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<MarketEntry>, String> {
    search(&provider, &query, limit.unwrap_or(30)).map_err(|e| e.to_string())
}

/// 榜单:board 仅对 skillssh 有区分 = "all" | "trending" | "hot"。
#[tauri::command]
pub fn market_leaderboard(provider: String, board: String) -> Result<Vec<MarketEntry>, String> {
    leaderboard(&provider, &board).map_err(|e| e.to_string())
}

/// 已装市场技能的登记表(前端用来标记"已安装")。
#[tauri::command]
pub fn market_registry() -> Result<Vec<MarketRecord>, String> {
    Ok(registry_list())
}

/// 从市场安装一个 skill:下载(克隆/zip)→ 复制进 market 目录 → 注册常驻
/// 来源 → 后台扫描。同步命令(跑在线程池,不卡 UI),网络操作受超时约束。
#[tauri::command]
pub fn install_market_skill(
    provider: String,
    source: String,
    skill_id: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<InstallOutcome, String> {
    // ① 慢路径无锁:下载/克隆 + 复制落盘 + 写登记表。
    let mut outcome = install_skill(&provider, &source, &skill_id).map_err(|e| e.to_string())?;

    // ② 短写锁:注册(或复用)市场来源;重装路径顺带把隐藏记录清掉,
    // 否则重装同路径的 skill 会被扫描器当作"用户已删除"而隐藏。
    let (src, newly_added) = {
        let mut guard = state.inner.write();
        let (src, newly) = ensure_market_source(&mut guard).map_err(|e| e.to_string())?;
        if outcome.replaced {
            guard
                .hidden_skills
                .remove(&format!("{}:{}", src.id, outcome.dir_name));
        }
        (src, newly)
    };
    outcome.source_id = src.id.to_string();
    state.save().map_err(|e| e.to_string())?;

    // ③ 新建来源时挂 watcher(与 add_local_source 一致,market 目录变化自动重扫)。
    if newly_added {
        if let Ok(root) = market_root() {
            crate::watcher::watch(app.clone(), src.id, root);
        }
    }

    // ④ 后台扫描 + 事件(前端已监听 source-scanned 刷新)。
    let state2 = state.inner().clone();
    let app2 = app.clone();
    let sid = src.id;
    std::thread::spawn(move || {
        crate::commands::run_scan_blocking(&app2, &state2, sid, "market-install".to_string());
        let _ = app2.emit("skill-dock://source-scanned", sid.to_string());
    });
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_encodes_reserved() {
        assert_eq!(urlencode("pdf"), "pdf");
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
        assert_eq!(urlencode("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn sanitize_dir_name_basic() {
        assert_eq!(sanitize_dir_name("react-pdf"), "react-pdf");
        assert_eq!(sanitize_dir_name("a b//c"), "a-b-c");
        assert_eq!(sanitize_dir_name("---"), "skill");
        assert_eq!(sanitize_dir_name(""), "skill");
    }

    #[test]
    fn validate_inputs() {
        assert!(validate_owner_repo("anthropics/skills").is_ok());
        assert!(validate_owner_repo("a..b/s_k-l.l").is_ok());
        assert!(validate_owner_repo("only-one").is_err());
        assert!(validate_owner_repo("a/b/c").is_err());
        assert!(validate_owner_repo("../etc").is_err());
        assert!(validate_owner_repo("a b/c").is_err());
        assert!(validate_skill_id("pdf").is_ok());
        assert!(validate_skill_id("../x").is_err());
        assert!(validate_skill_id("a/b").is_err());
        assert!(validate_skill_id("").is_err());
    }

    #[test]
    fn parses_initial_skills_payload() {
        // 模拟 RSC payload:JSON 在 JS 字符串里,引号被转义。
        let payload = r#"{"foo":"bar","initialSkills\":[{\"source\":\"a/b\",\"skillId\":\"pdf\",\"name\":\"pdf\",\"installs\":100,\"weeklyInstalls\":[1,2],\"isOfficial\":true},{\"source\":\"c/d\",\"skillId\":\"x]y\",\"name\":\"x]y\",\"installs\":2}],\"after\":1}"#;
        let entries = parse_initial_skills(payload).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, "a/b");
        assert_eq!(entries[0].skill_id, "pdf");
        assert_eq!(entries[0].installs, 100);
        assert!(entries[0].is_official);
        // 字符串里的 `]` 不得破坏括号平衡
        assert_eq!(entries[1].skill_id, "x]y");
    }

    #[test]
    fn parse_initial_skills_missing_marker() {
        assert!(parse_initial_skills("<html>nothing</html>").is_err());
    }

    #[test]
    fn finds_skill_in_known_layouts() {
        let root = unique_test_dir();
        let skill = root.join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: pdf\ndescription: d\n---\n").unwrap();

        assert_eq!(
            find_skill_dir(&root, "pdf").unwrap(),
            skill,
            "应命中 .agents/skills 布局"
        );
        assert!(find_skill_dir(&root, "nope").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finds_skill_by_recursive_name_and_frontmatter() {
        let root = unique_test_dir();
        // 目录名匹配(深埋 + 无 SKILL.md 的同名目录优先级低)
        let deep = root.join("docs/examples/tilde");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("SKILL.md"), "# tilde").unwrap();
        // frontmatter name 匹配(目录名不同)
        let fm_dir = root.join("weird-dir-name");
        std::fs::create_dir_all(&fm_dir).unwrap();
        std::fs::write(
            fm_dir.join("SKILL.md"),
            "---\nname: front-matter-skill\n---\n",
        )
        .unwrap();
        // 干扰项:.git 里的同名目录必须被跳过
        let git_dir = root.join(".git/pdf");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("SKILL.md"), "# evil").unwrap();

        assert_eq!(find_skill_dir(&root, "tilde").unwrap(), deep);
        assert_eq!(find_skill_dir(&root, "front-matter-skill").unwrap(), fm_dir);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_skips_dotted_entries_and_symlinks() {
        let root = unique_test_dir();
        let src = root.join("src-skill");
        std::fs::create_dir_all(&src.join(".git")).unwrap();
        std::fs::write(src.join(".git/config"), "x").unwrap();
        std::fs::write(src.join("SKILL.md"), "# s").unwrap();
        std::fs::write(src.join(".DS_Store"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", src.join("leak")).unwrap();

        let dst = root.join("dst-skill");
        let n = copy_skill_dir(&src, &dst).unwrap();
        assert_eq!(n, 1, "只应复制 SKILL.md");
        assert!(!dst.join(".git").exists());
        assert!(!dst.join("leak").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_roundtrip() {
        // 不碰真实应用数据目录：仅验证 serde 往返。
        let mut reg = BTreeMap::new();
        reg.insert(
            "pdf".to_string(),
            MarketRecord {
                provider: PROVIDER_SKILLHUB.into(),
                source: String::new(),
                skill_id: "pdf".into(),
                dir: "pdf".into(),
                commit: Some("abc".into()),
                installed_at: Utc::now().to_rfc3339(),
            },
        );
        let bytes = serde_json::to_vec_pretty(&reg).unwrap();
        let back: BTreeMap<String, MarketRecord> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back["pdf"].provider, PROVIDER_SKILLHUB);
        // 旧格式(无 provider 字段)应缺省为 skillssh。
        let old = serde_json::json!({
            "source": "anthropics/skills", "skill_id": "pdf", "dir": "pdf",
            "commit": null, "installed_at": "2026-01-01T00:00:00Z"
        });
        let legacy: MarketRecord = serde_json::from_value(old).unwrap();
        assert_eq!(legacy.provider, PROVIDER_SKILLSSH);
    }

    #[test]
    fn parses_clawhub_items() {
        let search = r#"{"results":[
            {"displayName":"Pdf","slug":"pdf","summary":"PDF toolkit","downloads":48201,
             "official":false,"ownerHandle":"awspace",
             "install":{"reference":"awspace/pdf"}},
            {"displayName":"T2","slug":"","summary":"","downloads":0,
             "install":{"reference":"othmanadi/planning-with-files"},
             "metrics":{"lifetimeInstalls":869}}
        ]}"#;
        let resp: ClawhubSearchResp = serde_json::from_str(search).unwrap();
        assert_eq!(resp.results.len(), 2);
        assert_eq!(resp.results[0].owner_and_slug().unwrap(), ("awspace".into(), "pdf".into()));
        // 无 ownerHandle/slug 时从 install.reference 兜底
        assert_eq!(
            resp.results[1].owner_and_slug().unwrap(),
            ("othmanadi".into(), "planning-with-files".into())
        );
        assert_eq!(resp.results[1].installs(), 869);

        let trending = r#"{"items":[{"displayName":"S","downloads":0,
            "install":{"reference":"a/b"},
            "native":{"skill":{"summary":"来自 native 的简介"}}}]}"#;
        let resp: ClawhubTrendingResp = serde_json::from_str(trending).unwrap();
        assert_eq!(resp.items[0].summary_text(), "来自 native 的简介");
    }

    #[test]
    fn parses_skillhub_resp() {
        let body = r#"{"code":0,"message":"","data":{"total":2,"skills":[
            {"slug":"pdf","name":"PDF 工具","description":"中文描述","downloads":223,"installs":0},
            {"slug":"","name":"无 slug 应被过滤","description":"","downloads":1,"installs":1}
        ]}}"#;
        let resp: SkillhubResp = serde_json::from_str(body).unwrap();
        assert_eq!(resp.code, 0);
        let data = resp.data.unwrap();
        assert_eq!(data.total, 2);
        assert_eq!(data.skills.len(), 2);
        assert_eq!(data.skills[0].slug, "pdf");
        assert_eq!(data.skills[0].description, "中文描述");
    }

    fn unique_test_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "skill-dock-market-{nanos}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// 端到端真装(skills.sh 路径,需网络):沙盒 HOME 下克隆
    /// vercel-labs/skills、定位 find-skills、复制进 market 目录、登记、
    /// 并按来源扫描出该 skill。
    /// 手动运行:`cargo test --lib market::tests::live_install_skillssh -- --ignored --test-threads 1`
    #[test]
    #[ignore]
    fn live_install_skillssh() {
        let sandbox = unique_test_dir();
        std::env::set_var("HOME", &sandbox);

        let outcome = install_skill(PROVIDER_SKILLSSH, "vercel-labs/skills", "find-skills")
            .expect("安装失败");
        assert!(!outcome.replaced);
        assert!(outcome.files > 0);
        assert!(
            market_root()
                .unwrap()
                .join(&outcome.dir_name)
                .join("SKILL.md")
                .is_file(),
            "SKILL.md 应已落盘"
        );

        let mut store = PersistedState::current();
        let (src, newly) = ensure_market_source(&mut store).unwrap();
        assert!(newly, "首次安装应新建「Skill 市场」来源");
        // 再走一遍 ensure 应复用同一来源。
        let (again, newly2) = ensure_market_source(&mut store).unwrap();
        assert!(!newly2 && again.id == src.id);

        let skills = crate::scanner::scan(&src, &market_root().unwrap(), std::path::Path::new("/nonexistent-vault")).unwrap();
        assert!(
            skills.iter().any(|s| s.name == outcome.name),
            "扫描结果应包含刚装的 skill,实际: {:?}",
            skills.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
        );

        let reg = read_registry().unwrap();
        assert_eq!(reg[&outcome.dir_name].provider, PROVIDER_SKILLSSH);

        std::fs::remove_dir_all(sandbox).ok();
    }

    /// 端到端真装(zip 路径,需网络):腾讯 SkillHub 下载 → 解压 → 落库,
    /// 覆盖 302 跟随 + archive 安全解压 + registry 的 provider 记录。
    #[test]
    #[ignore]
    fn live_install_skillhub_zip() {
        let sandbox = unique_test_dir();
        std::env::set_var("HOME", &sandbox);

        // 真实搜索一把拿个确定存在的 slug(pdf 生态里有名有姓的)。
        let entries = search(PROVIDER_SKILLHUB, "pdf", 5).expect("SkillHub 搜索失败");
        assert!(!entries.is_empty(), "搜索应至少命中一条");
        let entry = entries[0].clone();
        assert_eq!(entry.provider, PROVIDER_SKILLHUB);
        assert_eq!(entry.id, entry_id(PROVIDER_SKILLHUB, "", &entry.skill_id));

        let outcome = install_skill(PROVIDER_SKILLHUB, &entry.source, &entry.skill_id)
            .expect("SkillHub zip 安装失败");
        assert!(outcome.files > 0);
        assert!(
            market_root()
                .unwrap()
                .join(&outcome.dir_name)
                .join("SKILL.md")
                .is_file(),
            "SKILL.md 应已落盘"
        );
        let reg = read_registry().unwrap();
        let rec = &reg[&outcome.dir_name];
        assert_eq!(rec.provider, PROVIDER_SKILLHUB);
        // repos/zips 解压残留应清理干净
        let zips = repos_dir().unwrap().join("zips");
        if zips.is_dir() {
            let left: Vec<_> = std::fs::read_dir(&zips).unwrap().flatten().collect();
            assert!(left.is_empty(), "zips 解压残留: {:?}", left.iter().map(|e| e.path()).collect::<Vec<_>>());
        }
        std::fs::remove_dir_all(sandbox).ok();
    }

    /// 端到端真装(ClawHub zip 路径,需网络)。
    #[test]
    #[ignore]
    fn live_install_clawhub_zip() {
        let sandbox = unique_test_dir();
        std::env::set_var("HOME", &sandbox);

        let entries = search(PROVIDER_CLAWHUB, "pdf", 5).expect("ClawHub 搜索失败");
        assert!(!entries.is_empty(), "搜索应至少命中一条");
        let entry = entries[0].clone();
        assert!(!entry.source.is_empty(), "clawhub 条目应带作者 handle");

        let outcome = install_skill(PROVIDER_CLAWHUB, &entry.source, &entry.skill_id)
            .expect("ClawHub zip 安装失败");
        assert!(outcome.files > 0);
        let reg = read_registry().unwrap();
        assert_eq!(reg[&outcome.dir_name].provider, PROVIDER_CLAWHUB);
        std::fs::remove_dir_all(sandbox).ok();
    }
}
