// Shared types — mirror of Rust types in src-tauri/src/state.rs

export type SourceKind = "local" | "github";

/** Lifecycle of a GitHub source's local clone. Local sources are `ready`. */
export type CloneStatus = "pending" | "cloning" | "ready" | "failed";

/** How a source's skills are managed: independent (flat) or bundled (grouped). */
export type SourceMode = "flat" | "grouped";

export interface Source {
  id: string;
  kind: SourceKind;
  name: string;
  location: string;
  clone_path: string | null;
  branch: string | null;
  created_at: string;
  last_scanned_at: string | null;
  last_commit_sha: string | null;
  skill_count: number;
  clone_status: CloneStatus;
  clone_error: string | null;
  mode: SourceMode;
}

export interface Skill {
  id: string;
  source_id: string;
  name: string;
  description: string;
  relative_path: string;
  absolute_path: string;
  has_skill_md: boolean;
  content_hash: string;
  modified_at: string;
  size_bytes: number;
  is_shared: boolean;
  is_symlink: boolean;
  symlink_target: string | null;
}

export interface AgentTarget {
  id: string;
  name: string;
  /** 默认技能目录：新安装都落这里；内置 agent 可重置回内置值 */
  skills_dir: string;
  /** 额外技能地址：该 agent 也会从这里读技能（只观察，不接收安装） */
  extra_dirs: string[];
  enabled: boolean;
  description: string;
  tag: string | null;
  /** Runtime flag: agent's CLI or home dir was detected on this machine. */
  detected: boolean;
}

/** classify_target_path：添加/编辑 Agent 时对输入路径的实时识别结果。 */
export interface PathInsight {
  /** 展开 ~ 并尽量 canonicalize 后的绝对路径 */
  normalized: string;
  exists: boolean;
  is_dir: boolean;
  /** 目录内含 SKILL.md 的技能数；路径不存在或不是目录时为 null */
  skill_count: number | null;
  /** 命中内置 agent 目录时的 id（同时是图标 key） */
  matched_id: string | null;
  matched_name: string | null;
  matched_desc: string | null;
  matched_tag: string | null;
  /** "exact" = 内置目录本尊；"pattern" = 尾缀吻合（如项目级 .claude/skills） */
  matched_how: "exact" | "pattern" | null;
  /** 已被其他 Agent 占用时给出对方名称（路径唯一性预检） */
  used_by: string | null;
  used_by_id: string | null;
  /** true = 路径就是被编辑目标自己的默认目录（默认目录字段里算正常） */
  own_default: boolean;
}

/** list_builtin_dirs：内置 agent 的出厂默认目录（重置提示用）。 */
export interface BuiltinDirInfo {
  id: string;
  name: string;
  dir: string;
}

/** update_target 的返回：更新后的 target + 换目录时迁移的软链计数。 */
export interface TargetUpdateResult {
  target: AgentTarget;
  /** 已在新目录重建并删除旧链接的数量 */
  moved_links: number;
  /** 只改了台账指向、没能搬动的数量（旧位置是非软链内容等） */
  left_links: number;
  /** 被移除额外地址下的安装记录清理数（磁盘未动） */
  dropped_links: number;
}

export interface Installation {
  skill_id: string;
  target_id: string;
  link_path: string;
  installed_at: string;
  /**
   * 健康状态："ok" / "" = 正常；"missing" = 软链丢失；
   * "conflict" = 名字被非本程序的目录/链接占用。
   */
  status: string;
}

/** 组合——一组可整批安装/卸载的 skill 集合。 */
export interface SkillGroup {
  id: string;
  name: string;
  created_at: string;
  skill_ids: string[];
}

/** 项目工作区：注册的项目根目录（项目级 skill 管理）。 */
export interface Project {
  id: string;
  name: string;
  path: string;
  created_at: string;
  sort_order: number;
}

/** 五态同步健康度汇总。 */
export interface SyncHealth {
  in_sync: number;
  project_newer: number;
  center_newer: number;
  diverged: number;
  project_only: number;
}

/** 项目列表条目（含扫描汇总）。 */
export interface ProjectOverview {
  id: string;
  name: string;
  path: string;
  skill_count: number;
  health: SyncHealth;
}

/** 项目内一个技能实例（磁盘扫描 + 中央库匹配结果）。 */
export interface ProjectSkillInfo {
  name: string;
  description: string;
  /** 相对 skills 根的路径，"/" 分隔。 */
  relative_path: string;
  /** 所属槽位（agent 标识），如 ".claude/skills"。 */
  agent_dir: string;
  agent_names: string;
  path: string;
  enabled: boolean;
  is_symlink: boolean;
  /** in_sync | project_newer | center_newer | diverged | project_only */
  sync_status: string;
  center_skill_id: string | null;
  center_source_name: string | null;
}

/** 项目内可用的 agent 槽位（安装弹窗用）。 */
export interface ProjectSlotInfo {
  dir: string;
  display: string;
  exists: boolean;
}

/** 项目技能的 SKILL.md 内容。 */
export interface ProjectDoc {
  name: string;
  description: string;
  content: string;
}

/** 目标位四态分类（预览接口返回）。 */
export type CellState = "absent" | "linked" | "stale" | "occupied" | "blocked";

export interface PreviewCell {
  skill_id: string;
  skill_name: string;
  target_id: string;
  target_name: string;
  state: CellState;
  detail: string | null;
}

export interface CellFailure {
  skill_id: string;
  skill_name: string;
  target_id: string;
  target_name: string;
  error: string;
}

export interface BatchResult {
  installed: number;
  removed: number;
  failures: CellFailure[];
}

export interface ApplyOutcome extends BatchResult {
  skipped: number;
}

/** 详情页冲突弹窗：agent 安装位与托管目录的逐文件对比。 */
export interface DirDiffRow {
  path: string;
  status: "same" | "changed" | "only_source" | "only_target";
  source_size: number | null;
  source_mtime: number | null;
  target_size: number | null;
  target_mtime: number | null;
}

export interface SkillTargetDiff {
  occupant: "none" | "real_dir" | "real_file" | "symlink";
  /** 实际参与对比的对方目录（软链解析后）；无法对比时为 null。 */
  compared_dir: string | null;
  rows: DirDiffRow[];
  note: string | null;
}

export interface PendingEntry {
  skill_id: string;
  skill_name: string;
  target_id: string | null;
  target_name: string | null;
  detail: string;
}

export interface PendingReport {
  uninstalled: PendingEntry[];
  broken_links: PendingEntry[];
  outdated_sources: string[];
  undetected_targets: string[];
}

export interface UpdateEntry {
  source_id: string;
  source_name: string;
  source_kind: SourceKind;
  previous_skill_count: number;
  current_skill_count: number;
  added_skills: string[];
  removed_skills: string[];
  commit_changed: boolean;
  new_commit_sha: string | null;
  previous_commit_sha: string | null;
}

export interface UpdateReport {
  generated_at: string;
  updates: UpdateEntry[];
}

export interface StateSnapshot {
  sources: Source[];
  targets: AgentTarget[];
  installations: Installation[];
  skills: Skill[];
  groups: SkillGroup[];
  projects: Project[];
  data_dir: string;
  state_path: string;
  update_report: UpdateReport;
}

/** Raw filesystem entry in a target's skills_dir — what's actually on disk,
 *  including manually-created symlinks that SkillDock didn't install. */
export interface TargetSkillEntry {
  name: string;
  /** Absolute entry path under the Agent skills root. */
  path: string;
  is_symlink: boolean;
  symlink_target: string | null;
  has_skill_md: boolean;
  /**
   * `managed` is a live link created by this app, `external_symlink` is a
   * user/other-tool link, and `local` is a real directory in the target.
   */
  kind: SkillLocationKind;
}

export type SkillLocationKind = "local" | "managed" | "external_symlink";

/** A raw skill discovered from one of the configured Agent targets. */
export interface DiscoveredAgentSkill extends TargetSkillEntry {
  target_id: string;
  target_name: string;
  target_skills_dir: string;
}

/** A node in a recursive file tree (used by the agent-skills file browser). */
export interface FileTreeNode {
  name: string;
  /** Path relative to the tree root (forward-slash separated). */
  path: string;
  is_dir: boolean;
  children: FileTreeNode[];
}

export interface SkillContent {
  skill_id: string;
  exists: boolean;
  content: string;
  path: string;
}

/** 来源更新的文件级变更。 */
export interface SourceFileChange {
  /** added | modified | deleted | renamed */
  status: string;
  path: string;
  new_path: string | null;
}

export interface SourceUpdatePreview {
  source_id: string;
  name: string;
  branch: string;
  current_sha: string | null;
  remote_sha: string | null;
  has_update: boolean;
  files: SourceFileChange[];
  affected_skills: string[];
  truncated: boolean;
  /** 本地落后远端的提交数；null = 统计不出来 */
  behind_count: number | null;
  /** false = 计数被加深上限截断，应显示为 N+ */
  behind_exact: boolean;
  /** 远端新提交主题（最新在前，最多 10 条） */
  recent_commits: string[];
  note: string | null;
}

/** 按磁盘路径反查到的库内 skill。 */
export interface SkillMatch {
  id: string;
  name: string;
}

/** 本机扫描：某真实 skill 在一个 Agent 目录里的落点。 */
export interface DiscoveredLink {
  target_id: string;
  target_name: string;
  entry_name: string;
  link_path: string;
  is_symlink: boolean;
}

/** 本机扫描：一个真实 skill（按真实文件聚合的组）。 */
export interface DiscoveredSkillGroup {
  real_path: string;
  name: string;
  has_skill_md: boolean;
  /** 已纳管时 = 技能库中对应 skill 的 id（详情路由用）；未纳管 = null。 */
  skill_id: string | null;
  /** 已纳管时 = 技能库中对应来源；未纳管 = null。 */
  source_id: string | null;
  source_name: string | null;
  links: DiscoveredLink[];
}

/** 未纳管 skill 的详情元数据（按磁盘路径读取）。 */
export interface LocalSkillMeta {
  name: string;
  description: string;
  size_bytes: number;
  /** SKILL.md 的修改时间（ISO 8601）。 */
  modified_at: string | null;
}

/** 收编结果。 */
export interface AdoptResult {
  new_path: string;
  repointed: number;
  moved_original: number;
  source_id: string;
  source_registered: boolean;
}

/** skill 市场条目 —— 多市场源统一结构。 */
export type MarketProvider = "skillssh" | "clawhub" | "skillhub";

export interface MarketEntry {
  provider: MarketProvider;
  /** 全局唯一键 "{provider}:{source}/{skill_id}",已安装标记用它。 */
  id: string;
  /** slug / 目录名。 */
  skill_id: string;
  name: string;
  /** skills.sh / clawhub 为仓库或作者 handle;skillhub 为空。 */
  source: string;
  installs: number;
  /** 一句话简介(skillssh 无此数据,为空串)。 */
  description: string;
  is_official: boolean;
}

/** 已装市场技能的登记记录(后端 market/.market-registry.json)。 */
export interface MarketRecord {
  provider: MarketProvider;
  source: string;
  skill_id: string;
  /** market 根目录下的目录名。 */
  dir: string;
  commit: string | null;
  installed_at: string;
}

/** 市场安装结果。 */
export interface MarketInstallOutcome {
  source_id: string;
  skill_id: string;
  dir_name: string;
  name: string;
  /** true = 同源重装(覆盖更新);false = 首次安装。 */
  replaced: boolean;
  files: number;
}
