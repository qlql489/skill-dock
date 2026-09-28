// Thin wrapper around Tauri IPC commands.
// Centralized so views don't import @tauri-apps/api directly.

import { invoke } from "@tauri-apps/api/core";
import { open as shellOpen } from "@tauri-apps/plugin-shell";
import type {
  StateSnapshot,
  Source,
  Skill,
  AgentTarget,
  Installation,
  UpdateReport,
  SkillContent,
  TargetSkillEntry,
  DiscoveredAgentSkill,
  FileTreeNode,
  SkillGroup as SkillGroupT,
  PreviewCell,
  BatchResult,
  ApplyOutcome,
  SkillTargetDiff,
  PendingReport as PendingReportT,
  SourceUpdatePreview as SourceUpdatePreviewT,
  SkillMatch,
  DiscoveredSkillGroup,
  AdoptResult,
  LocalSkillMeta,
  Project as ProjectT,
  ProjectOverview as ProjectOverviewT,
  ProjectSkillInfo as ProjectSkillInfoT,
  ProjectSlotInfo as ProjectSlotInfoT,
  ProjectDoc as ProjectDocT,
  MarketEntry as MarketEntryT,
  MarketProvider as MarketProviderT,
  MarketRecord as MarketRecordT,
  MarketInstallOutcome as MarketInstallOutcomeT,
  PathInsight as PathInsightT,
  TargetUpdateResult as TargetUpdateResultT,
  BuiltinDirInfo as BuiltinDirInfoT,
} from "./types";

export const api = {
  getState:        () => invoke<StateSnapshot>("get_state"),
  getStatePath:    () => invoke<string>("get_state_path"),

  addLocalSource:  (name: string, location: string, mode: "flat" | "grouped" = "flat", autoSyncTargets: string[] = [], copyToVault = false) =>
    invoke<Source>("add_local_source", { name, location, mode, autoSyncTargets, copyToVault }),

  addGithubSource: (name: string, url: string, mode: "flat" | "grouped" = "flat", branch?: string | null) =>
    invoke<Source>("add_github_source", { name, url, branch: branch ?? null, mode }),

  retryCloneSource: (id: string) =>
    invoke<void>("retry_clone_source", { id }),

  removeSource:    (id: string) => invoke<void>("remove_source", { id }),
  rescanSource:    (id: string) => invoke<Skill[]>("rescan_source", { id }),
  rescanAll:       () => invoke<number>("rescan_all"),
  scanAll:         () =>
    invoke<{ total_skills: number; per_source: [string, number][] }>("scan_all"),

  readSkillMd:     (skillId: string) =>
    invoke<SkillContent>("read_skill_md", { req: { skill_id: skillId } }),

  writeSkillMd:    (skillId: string, content: string) =>
    invoke<void>("write_skill_md", { req: { skill_id: skillId, content } }),

  listTargets:     () => invoke<AgentTarget[]>("list_targets"),

  listTargetSkills: (targetId: string) =>
    invoke<TargetSkillEntry[]>("list_target_skills", { targetId }),

  /** Read-only inventory of the skills already present in all Agent targets. */
  discoverAgentSkills: () =>
    invoke<DiscoveredAgentSkill[]>("discover_existing_skills"),

  readSkillTree: (rootPath: string) =>
    invoke<FileTreeNode[]>("read_skill_tree", { rootPath }),

  readSkillFile: (rootPath: string, filePath: string) =>
    invoke<string>("read_skill_file", { rootPath, filePath }),

  redetectTargets: () => invoke<number>("redetect_targets"),

  addTarget: (name: string, skillsDir: string, description: string, tag: string | null) =>
    invoke<AgentTarget>("add_target", {
      req: { name, skills_dir: skillsDir, description, tag },
    }),

  classifyTargetPath: (path: string, excludeId?: string | null) =>
    invoke<PathInsightT>("classify_target_path", {
      req: { path, exclude_id: excludeId ?? null },
    }),

  classifyBuiltinDirs: () => invoke<BuiltinDirInfoT[]>("list_builtin_dirs"),

  resetTargetDir: (id: string) =>
    invoke<TargetUpdateResultT>("reset_target_dir", { id }),

  updateTarget: (
    id: string,
    enabled: boolean | null,
    name: string | null,
    skillsDir: string | null,
    extraDirs: string[] | null,
    description: string | null,
    tag: string | null,
  ) =>
    invoke<TargetUpdateResultT>("update_target", {
      req: {
        id,
        enabled,
        name,
        skills_dir: skillsDir,
        extra_dirs: extraDirs,
        description,
        tag: tag === null ? null : tag,
      },
    }),

  removeTarget:    (id: string) => invoke<void>("remove_target", { id }),

  installSkill:    (skillId: string, targetId: string, onConflict: "fail" | "replace" = "fail") =>
    invoke<Installation>("install_skill", {
      req: { skill_id: skillId, target_id: targetId, on_conflict: onConflict },
    }),

  uninstallSkill:  (skillId: string, targetId: string) =>
    invoke<void>("uninstall_skill", {
      req: { skill_id: skillId, target_id: targetId },
    }),

  diffSkillTarget: (skillId: string, targetId: string) =>
    invoke<SkillTargetDiff>("diff_skill_target", {
      req: { skill_id: skillId, target_id: targetId },
    }),

  deleteSkill:     (skillId: string) =>
    invoke<void>("delete_skill", { req: { skill_id: skillId } }),

  /** 真实删除：卸掉全部软链 + 磁盘目录进废纸篓。返回移除的软链数。 */
  deleteSkillForever: (skillId: string) =>
    invoke<number>("delete_skill_forever", { req: { skill_id: skillId } }),

  installSourceGroup: (sourceId: string, targetId: string) =>
    invoke<{ installed: number; failed: string[] }>("install_source_group", {
      req: { source_id: sourceId, target_id: targetId },
    }),

  uninstallSourceGroup: (sourceId: string, targetId: string) =>
    invoke<number>("uninstall_source_group", {
      req: { source_id: sourceId, target_id: targetId },
    }),

  updateSourceGroup: (sourceId: string) =>
    invoke<void>("update_source_group", {
      req: { source_id: sourceId, target_id: "" },
    }),

  deleteSourceGroup: (sourceId: string) =>
    invoke<number>("delete_source_group", { sourceId }),

  setSourceMode: (sourceId: string, mode: "flat" | "grouped") =>
    invoke<void>("set_source_mode", { req: { source_id: sourceId, mode } }),

  setSourceAutoSync: (sourceId: string, targetIds: string[]) =>
    invoke<void>("set_source_auto_sync", { req: { source_id: sourceId, target_ids: targetIds } }),

  checkUpdates:    () => invoke<UpdateReport>("check_updates"),

  applyUpdates:    (sourceId: string) =>
    invoke<void>("apply_updates", { req: { source_id: sourceId } }),

  revealInFinder:  (path: string) => invoke<void>("reveal_in_finder", { path }),
  openPath:        (path: string) => invoke<void>("open_path", { path }),
  openExternal:    (url: string) => shellOpen(url),

  pickFolder:      () => invoke<string | null>("pick_folder"),

  // ----- 组合 / 批量 / 预览 / 标签（groups.rs）-----

  createGroup:     (name: string) => invoke<SkillGroupT>("create_group", { name }),
  updateGroup:     (id: string, name?: string | null, skillIds?: string[] | null) =>
    invoke<SkillGroupT>("update_group", { req: { id, name: name ?? null, skill_ids: skillIds ?? null } }),
  deleteGroup:     (id: string) => invoke<void>("delete_group", { id }),
  applyGroup:      (groupId: string, targetIds: string[], activate: boolean) =>
    invoke<ApplyOutcome>("apply_group", { req: { group_id: groupId, target_ids: targetIds, activate } }),
  previewCells:    (skillIds: string[], targetIds: string[]) =>
    invoke<PreviewCell[]>("preview_cells", { req: { skill_ids: skillIds, target_ids: targetIds } }),
  applyCells:      (
    install: [string, string][],
    remove: [string, string][],
    replace: [string, string][] = [],
  ) =>
    invoke<BatchResult>("apply_cells", { req: { install, remove, replace } }),
  hideSourceSkills:   (sourceId: string, hideSkillPaths: string[]) =>
    invoke<void>("hide_source_skills", { req: { source_id: sourceId, hide_skill_paths: hideSkillPaths } }),
  unhideSourceSkills: (sourceId: string) => invoke<void>("unhide_source_skills", { sourceId }),
  pendingItems:    () => invoke<PendingReportT>("pending_items"),
  cleanupInstallations: (statuses: string[]) => invoke<number>("cleanup_installations", { statuses }),

  addZipSource:    (name: string, zipPath: string) =>
    invoke<Source>("add_zip_source", { name, zipPath }),

  /** 拖拽导入：前端拿不到拖入文件的磁盘路径，直接传字节（base64）。 */
  addZipBytes:     (name: string, fileName: string, dataB64: string) =>
    invoke<Source>("add_zip_bytes", { name, fileName, dataB64 }),

  pickArchiveFile: () => invoke<string | null>("pick_archive_file"),

  sourceUpdatePreview: (id: string) =>
    invoke<SourceUpdatePreviewT>("preview_source_update", { id }),
  sourceFilePatch: (sourceId: string, path: string) =>
    invoke<string>("source_file_patch", { req: { source_id: sourceId, path } }),

  resolveSkillByPath: (path: string) =>
    invoke<SkillMatch | null>("resolve_skill_by_path", { path }),

  discoverLocalSkills: (includeDisabled = false) =>
    invoke<DiscoveredSkillGroup[]>("discover_local_skills", { includeDisabled }),
  adoptLocalSkill: (realPath: string, destSourceId: string | null) =>
    invoke<AdoptResult>("adopt_local_skill", { req: { real_path: realPath, dest_source_id: destSourceId } }),
  /** 未纳管 skill 的详情元数据（按磁盘路径）。 */
  localSkillMeta: (rootPath: string) =>
    invoke<LocalSkillMeta>("local_skill_meta", { rootPath }),

  reorderTargets: (orderedIds: string[]) =>
    invoke<void>("reorder_targets", { orderedIds }),

  getAccentColor: () => invoke<string>("get_accent_color"),
  setAccentColor: (color: string) => invoke<void>("set_accent_color", { color }),

  getAddDefaultInstallTargets: () => invoke<boolean>("get_add_default_install_targets"),
  setAddDefaultInstallTargets: (enabled: boolean) =>
    invoke<void>("set_add_default_install_targets", { enabled }),

  /** 收编默认目的地（中央仓库）。返回生效路径（配置值或内置默认）。 */
  getVaultPath: () => invoke<string>("get_vault_path"),
  /** 空串 = 恢复内置默认中央仓库；支持 ~ 开头。 */
  setVaultPath: (path: string) => invoke<void>("set_vault_path", { path }),

  // ----- 项目级 skill 管理(projects.rs)-----

  getProjects: () => invoke<ProjectOverviewT[]>("get_projects"),
  addProject: (path: string) => invoke<ProjectT>("add_project", { path }),
  removeProject: (id: string) => invoke<void>("remove_project", { id }),
  scanProjectRoots: (root: string, depth = 4) =>
    invoke<string[]>("scan_project_roots", { root, depth }),
  getProjectSlots: (projectId: string) =>
    invoke<ProjectSlotInfoT[]>("get_project_slots", { projectId }),
  getProjectSkills: (projectId: string) =>
    invoke<ProjectSkillInfoT[]>("get_project_skills", { projectId }),
  getProjectSkillDoc: (projectId: string, agentDir: string, relativePath: string) =>
    invoke<ProjectDocT>("get_project_skill_doc", { projectId, agentDir, relativePath }),
  toggleProjectSkill: (projectId: string, agentDir: string, relativePath: string, enabled: boolean) =>
    invoke<void>("toggle_project_skill", {
      req: { project_id: projectId, agent_dir: agentDir, relative_path: relativePath, enabled },
    }),
  deleteProjectSkill: (projectId: string, agentDir: string, relativePath: string) =>
    invoke<void>("delete_project_skill", {
      req: { project_id: projectId, agent_dir: agentDir, relative_path: relativePath },
    }),
  adoptProjectSkill: (projectId: string, agentDir: string, relativePath: string) =>
    invoke<{ new_path: string; source_id: string; source_registered: boolean; already_in_vault: boolean }>(
      "adopt_project_skill",
      { req: { project_id: projectId, agent_dir: agentDir, relative_path: relativePath } },
    ),
  exportSkillToProject: (skillId: string, projectId: string, agentDir: string, mode: "copy" | "symlink" = "copy") =>
    invoke<string>("export_skill_to_project", {
      req: { skill_id: skillId, project_id: projectId, agent_dir: agentDir, mode },
    }),
  updateProjectSkillFromCenter: (projectId: string, agentDir: string, relativePath: string) =>
    invoke<void>("update_project_skill_from_center", {
      req: { project_id: projectId, agent_dir: agentDir, relative_path: relativePath },
    }),

  // ----- Skill 市场(skills.sh / 腾讯 SkillHub / ClawHub)-----

  marketSearch:     (provider: MarketProviderT, query: string, limit = 30) =>
    invoke<MarketEntryT[]>("market_search", { provider, query, limit }),
  marketLeaderboard: (provider: MarketProviderT, board: "all" | "trending" | "hot") =>
    invoke<MarketEntryT[]>("market_leaderboard", { provider, board }),
  marketRegistry:   () => invoke<MarketRecordT[]>("market_registry"),
  installMarketSkill: (provider: MarketProviderT, source: string, skillId: string) =>
    invoke<MarketInstallOutcomeT>("install_market_skill", { provider, source, skillId }),
};
