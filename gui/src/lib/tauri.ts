// Tauri IPC types and invoke helpers
import { invoke } from "@tauri-apps/api/core";

// === Types matching Rust structs ===

export interface TaskInfo {
  id: string;
  prompt: string | null;
  tool: string | null;
  started_at: string;
  completed_at: string | null;
  status: "active" | "completed" | "rolled-back" | "partial-rolled-back";
  risk_level: string | null;
  summary: string | null;
  changes_count: number;
  cwd: string | null;
  total_lines_added: number;
  total_lines_removed: number;
}

export interface ChangeInfo {
  id: number;
  task_id: string;
  file_path: string;
  change_type: "created" | "modified" | "deleted" | "renamed";
  old_hash: string | null;
  new_hash: string | null;
  diff_text: string | null;
  lines_added: number;
  lines_removed: number;
  /** ISO-8601 timestamp when this file was individually restored, or null. */
  restored_at: string | null;
  attribution: string | null;
}

export interface TaskStatsInfo {
  task_id: string;
  model: string | null;
  duration_secs: number | null;
  tool_calls: number;
  files_changed: number;
  input_tokens: number | null;
  output_tokens: number | null;
  total_cost_usd: number | null;
  session_id: string | null;
  conversation_id: string | null;
}

export interface ChangeDiffResult {
  diff_text: string | null;
  lines_added: number;
  lines_removed: number;
}

export interface UndoPreviewInfo {
  task_id: string;
  total_changes: number;
  files_to_restore: string[];
  files_to_delete: string[];
}

export interface UndoResultInfo {
  files_restored: number;
  files_deleted: number;
  failures: [string, string][];
}

export interface StatusInfo {
  running: boolean;
  paused: boolean;
  watch_dirs: string[];
  total_snapshots: number;
  anomaly_count: number;
  has_warning: boolean;
}

export interface ConfigInfo {
  watch_dirs: string[];
  ignore_patterns: string[];
}

export interface DirScanStatus {
  path: string;
  name: string;
  status: "pending" | "scanning" | "complete";
  files_total: number;
  files_done: number;
  files_failed: number;
  percent: number;
  last_completed_at: string | null;
}

export interface ScanProgressInfo {
  is_scanning: boolean;
  current_dir: string | null;
  dirs: DirScanStatus[];
}

// === API functions ===

export async function listTasks(dirFilter?: string): Promise<TaskInfo[]> {
  return invoke<TaskInfo[]>("list_tasks", { dirFilter: dirFilter ?? null });
}

export async function getTask(taskId: string): Promise<TaskInfo> {
  return invoke<TaskInfo>("get_task", { taskId });
}

export async function getTaskChanges(taskId: string, dirFilter?: string): Promise<ChangeInfo[]> {
  return invoke<ChangeInfo[]>("get_task_changes", { taskId, dirFilter: dirFilter ?? null });
}

export async function getChangeDiff(taskId: string, filePath: string): Promise<ChangeDiffResult> {
  return invoke<ChangeDiffResult>("get_change_diff", { taskId, filePath });
}

// Rollback (canonical V3 names)
export async function previewRollback(taskId: string): Promise<UndoPreviewInfo> {
  return invoke<UndoPreviewInfo>("preview_rollback", { taskId });
}

export async function rollbackTask(taskId: string): Promise<UndoResultInfo> {
  return invoke<UndoResultInfo>("rollback_task_cmd", { taskId });
}

export async function restoreFile(taskId: string, filePath: string): Promise<UndoResultInfo> {
  return invoke<UndoResultInfo>("restore_file_cmd", { taskId, filePath });
}

// Legacy aliases (kept so any callers don't break)
export async function previewUndo(taskId: string): Promise<UndoPreviewInfo> {
  return previewRollback(taskId);
}

export async function undoTask(taskId: string): Promise<UndoResultInfo> {
  return rollbackTask(taskId);
}

export async function undoFile(taskId: string, filePath: string): Promise<UndoResultInfo> {
  return restoreFile(taskId, filePath);
}

// Monitoring window config
export async function getMonitoringWindow(): Promise<number> {
  return invoke<number>("get_monitoring_window");
}

export async function setMonitoringWindow(secs: number): Promise<void> {
  return invoke("set_monitoring_window", { secs });
}

export async function getStatus(): Promise<StatusInfo> {
  return invoke<StatusInfo>("get_status");
}

export async function getConfig(): Promise<ConfigInfo> {
  return invoke<ConfigInfo>("get_config");
}

export async function checkFirstRun(): Promise<boolean> {
  return invoke<boolean>("check_first_run");
}

export async function completeSetup(watchDirs: string[]): Promise<void> {
  return invoke("complete_setup", { watchDirs });
}

export async function setPaused(paused: boolean): Promise<void> {
  return invoke("set_paused", { paused });
}

export async function getScanProgress(): Promise<ScanProgressInfo> {
  return invoke<ScanProgressInfo>("get_scan_progress");
}

export async function addWatchDir(dirPath: string): Promise<void> {
  return invoke("add_watch_dir", { dirPath });
}

export async function removeWatchDir(dirPath: string): Promise<void> {
  return invoke("remove_watch_dir", { dirPath });
}

export interface DirIgnoreConfigInfo {
  exclude_dirs: string[];
  exclude_extensions: string[];
}

export async function getDirIgnoreConfig(dirPath: string): Promise<DirIgnoreConfigInfo> {
  return invoke<DirIgnoreConfigInfo>("get_dir_ignore_config", { dirPath });
}

export async function updateDirIgnoreConfig(
  dirPath: string,
  excludeDirs: string[],
  excludeExtensions: string[],
): Promise<void> {
  return invoke("update_dir_ignore_config", { dirPath, excludeDirs, excludeExtensions });
}

export async function listSubdirs(dirPath: string): Promise<string[]> {
  return invoke<string[]>("list_subdirs", { dirPath });
}

export interface DirContentItem {
  name: string;
  full_path: string;
  is_dir: boolean;
  modified_secs: number;
}

export async function listDirContents(dirPath: string): Promise<DirContentItem[]> {
  return invoke<DirContentItem[]>("list_dir_contents", { dirPath });
}

export async function rescanWatchDir(dirPath: string): Promise<void> {
  return invoke("rescan_watch_dir", { dirPath });
}

export interface IgnoreConfigInfo {
  ignore_patterns: string[];
  max_file_size_bytes: number | null;
}

export async function getIgnoreConfig(): Promise<IgnoreConfigInfo> {
  return invoke<IgnoreConfigInfo>("get_ignore_config");
}

export async function updateIgnoreConfig(
  ignorePatterns: string[],
  sizeLimitBytes: number | null,
): Promise<void> {
  return invoke("update_ignore_config", { ignorePatterns, sizeLimitBytes });
}

export interface CategoryStats {
  category: string;
  extensions: string;
  file_count: number;
  total_bytes: number;
}

export interface DirAnalysis {
  path: string;
  total_files: number;
  total_bytes: number;
  categories: CategoryStats[];
  large_file_count: number;
  large_file_bytes: number;
}

export interface FullAnalysis {
  dirs: DirAnalysis[];
  total_files: number;
  total_bytes: number;
  categories: CategoryStats[];
  large_file_count: number;
  large_file_bytes: number;
}

export async function analyzeDirectories(): Promise<FullAnalysis> {
  return invoke<FullAnalysis>("analyze_directories");
}

export interface StorageInfo {
  object_count: number;
  apparent_bytes: number;
  note: string;
}

export async function getStorageInfo(): Promise<StorageInfo> {
  return invoke<StorageInfo>("get_storage_info");
}

// === AI Tool Hook Management ===

export interface AiToolInfo {
  id: string;
  name: string;
  hook_installed: boolean;
  config_path: string | null;
}

export async function detectAiTools(): Promise<AiToolInfo[]> {
  return invoke<AiToolInfo[]>("detect_ai_tools");
}

export async function installToolHook(toolId: string): Promise<void> {
  return invoke("install_tool_hook", { toolId });
}

export async function uninstallToolHook(toolId: string): Promise<void> {
  return invoke("uninstall_tool_hook", { toolId });
}

// === Task Statistics ===

export async function getTaskStats(taskId: string): Promise<TaskStatsInfo | null> {
  return invoke<TaskStatsInfo | null>("get_task_stats", { taskId });
}
