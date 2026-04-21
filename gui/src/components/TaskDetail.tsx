import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { RotateCcw, ChevronDown, ChevronRight, Clock, StopCircle } from "lucide-react";
import { useTaskChanges } from "../hooks/useTasks";
import {
  type TaskInfo,
  type ChangeInfo,
  type TaskStatsInfo,
  type RestoreProgressInfo,
  type RestoreOperationInfo,
  getTask,
  getTaskStats,
  getRestoreProgress,
  listRestoreOperations,
  restoreFile,
  restoreDirectory,
  getChangeDiff,
  getObjectBase64,
  stopTask,
} from "../lib/tauri";
import { timeAgo, fileName, dirName } from "../lib/format";
import { getToolMeta } from "../lib/tools";
import DiffViewer from "./DiffViewer";
import RollbackPanel from "./RollbackPanel";
import RestoreHistoryModal from "./RestoreHistoryModal";

// Format duration: e.g. 90s → "1m30s", 60s → "1m"
function formatDuration(secs: number): string {
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  const s = Math.round(secs % 60);
  return s > 0 ? `${m}m${s}s` : `${m}m`;
}

function restorePhaseLabel(phase: RestoreProgressInfo["phase"]): string {
  switch (phase) {
    case "restoring-files":
      return "正在恢复文件";
    case "syncing-database":
      return "正在同步数据库记录";
    case "finalizing":
      return "正在收尾";
    case "done":
      return "已完成";
    default:
      return "准备中";
  }
}

function restoreButtonLabel(progress: RestoreProgressInfo | null): string {
  if (!progress || !progress.is_running) return "恢复中...";
  switch (progress.phase) {
    case "syncing-database":
      return "同步记录中...";
    case "finalizing":
      return "收尾中...";
    default:
      return "恢复中...";
  }
}

interface Props {
  taskId: string;
  dirFilter?: string | null;
  onTaskUpdated: () => void;
  onBack: () => void;
}

function isMonitoringWindow(task: TaskInfo): boolean {
  return task.tool === "文件监听";
}

function windowLabel(task: TaskInfo): string {
  const fmt = (iso: string) => {
    const d = new Date(iso);
    return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  };
  const start = fmt(task.started_at);
  if (task.completed_at) {
    const end = fmt(task.completed_at);
    return start === end ? start : `${start} – ${end}`;
  }
  return start;
}

function effectiveTs(task: TaskInfo): string {
  return isMonitoringWindow(task)
    ? (task.completed_at ?? task.started_at)
    : task.started_at;
}

const DOT_COLOR: Record<string, string> = {
  created: "bg-sys-green",
  modified: "bg-sys-amber",
  deleted: "bg-sys-red",
  renamed: "bg-sys-blue",
};

export default function TaskDetail({ taskId, dirFilter, onTaskUpdated, onBack }: Props) {
  const [task, setTask] = useState<TaskInfo | null>(null);
  const [stats, setStats] = useState<TaskStatsInfo | null>(null);
  const {
    changes,
    deletedDirs,
    totalCount,
    truncated,
    loading: changesLoading,
    refresh: refreshChanges,
  } = useTaskChanges(taskId, dirFilter);
  const [showRollback, setShowRollback] = useState(false);
  const [confirmDirRestore, setConfirmDirRestore] = useState<string | null>(null);
  const [dirRestoreBusy, setDirRestoreBusy] = useState(false);
  const [dirRestoreError, setDirRestoreError] = useState<string | null>(null);
  const [restoreProgress, setRestoreProgress] = useState<RestoreProgressInfo | null>(null);
  const [restoreOperations, setRestoreOperations] = useState<RestoreOperationInfo[]>([]);
  const [showRestoreHistory, setShowRestoreHistory] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);

  // Which file is expanded to show diff (null = none)
  const [expandedFilePath, setExpandedFilePath] = useState<string | null>(null);

  useEffect(() => {
    getTask(taskId).then(setTask).catch(() => setTask(null));
    getTaskStats(taskId).then(setStats).catch(() => setStats(null));
    listRestoreOperations(taskId).then(setRestoreOperations).catch(() => setRestoreOperations([]));
  }, [taskId]);

  // Reset expanded file when task changes
  useEffect(() => {
    setExpandedFilePath(null);
    setConfirmDirRestore(null);
    setDirRestoreBusy(false);
    setDirRestoreError(null);
    setRestoreProgress(null);
    setRestoreOperations([]);
    setConfirmStop(false);
    setStopping(false);
  }, [taskId]);

  useEffect(() => {
    getRestoreProgress().then(setRestoreProgress).catch(() => {});
    const unlisten = listen<RestoreProgressInfo>("restore-progress", (event) => {
      setRestoreProgress(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  if (!task) {
    return (
      <div className="h-full flex items-center justify-center text-t-3 text-[13px]">
        <span className="animate-spin mr-2">◐</span> 加载中...
      </div>
    );
  }

  const isWindow = isMonitoringWindow(task);
  const isRolledBack = task.status === "rolled-back";
  const isActive = task.status === "active";
  const loadedAdded = changes.reduce((s, c) => s + c.lines_added, 0);
  const loadedRemoved = changes.reduce((s, c) => s + c.lines_removed, 0);
  const displayChangeCount = dirFilter ? totalCount : task.changes_count;
  const totalAdded = dirFilter ? loadedAdded : task.total_lines_added;
  const totalRemoved = dirFilter ? loadedRemoved : task.total_lines_removed;
  const onlyDeletedChangesLoaded = changes.length > 0 && changes.every((change) => change.change_type === "deleted");
  const visibleDeletedDirGroups = deletedDirs.slice(0, 5);

  const ts = new Date(effectiveTs(task));
  const timeString = ts.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
  const dateString = ts.toLocaleDateString("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit" });

  // Fallback: compute duration from task timestamps when stop hook didn't fire.
  // For in-progress tasks use elapsed time since started_at.
  const displayDurationSecs: number | null = (() => {
    if (stats?.duration_secs != null && stats.duration_secs > 0) return stats.duration_secs;
    const endMs = task.completed_at
      ? new Date(task.completed_at).getTime()
      : Date.now();
    const secs = (endMs - new Date(task.started_at).getTime()) / 1000;
    return secs > 0 ? secs : null;
  })();

  const toggleFile = (filePath: string) => {
    setExpandedFilePath(expandedFilePath === filePath ? null : filePath);
  };

  const handleDirectoryRestore = async (dirPath: string) => {
    setDirRestoreBusy(true);
    setDirRestoreError(null);
    try {
      await restoreDirectory(taskId, dirPath);
      setConfirmDirRestore(null);
      getRestoreProgress().then(setRestoreProgress).catch(() => {});
      refreshChanges();
      listRestoreOperations(taskId).then(setRestoreOperations).catch(() => {});
      onTaskUpdated();
      getTask(taskId).then(setTask);
    } catch (err) {
      setDirRestoreError(String(err));
      setTimeout(() => setDirRestoreError(null), 4000);
    } finally {
      setDirRestoreBusy(false);
    }
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex-shrink-0 px-5 py-3 border-b border-border">
        <div className="flex items-center justify-between">
          <div>
            <div className="text-[11px] font-semibold text-t-3 uppercase tracking-wider mb-1">
              Selected Point
              {isRolledBack && <span className="ml-2 text-sys-red normal-case">· 已读档</span>}
            </div>
            <div className="text-[18px] font-bold text-t-1 tabular-nums leading-tight">
              {timeString}
            </div>
            <div className="text-[13px] text-t-2 tabular-nums">{dateString}</div>
            {(task.finalization_status === "pending" || task.finalization_status === "running") && (
              <div className="text-[12px] text-sys-blue mt-1">正在整理变更...</div>
            )}
            {task.finalization_status === "failed" && (
              <div className="text-[12px] text-sys-red mt-1">变更整理失败，稍后会自动重试</div>
            )}
          </div>

          <div className="flex items-center gap-2">
            {(() => {
              // Active task: no restore; show stop only after 20 min
              if (isActive) {
                const elapsed = Date.now() - new Date(task.started_at).getTime();
                if (elapsed <= 20 * 60 * 1000) return null;

                if (confirmStop) {
                  return (
                    <div className="flex flex-col items-end gap-1.5">
                      <div className="flex items-center gap-2">
                        <button
                          onClick={() => setConfirmStop(false)}
                          className="px-3 py-1.5 rounded-md text-[13px] font-medium bg-[rgba(0,0,0,0.06)] text-t-2 hover:bg-[rgba(0,0,0,0.1)] transition-colors"
                        >
                          取消
                        </button>
                        <button
                          disabled={stopping}
                          onClick={async () => {
                            setStopping(true);
                            try {
                              await stopTask(taskId);
                              onTaskUpdated();
                              getTask(taskId).then(setTask);
                            } catch (err) {
                              console.error("stop task failed:", err);
                            } finally {
                              setStopping(false);
                              setConfirmStop(false);
                            }
                          }}
                          className="flex items-center gap-2 px-4 py-1.5 rounded-md text-[13px] font-medium transition-colors disabled:opacity-50 bg-sys-red text-white hover:bg-sys-red/90 active:bg-sys-red/80"
                        >
                          <StopCircle className="w-[13px] h-[13px]" />
                          {stopping ? "终止中..." : "确认终止"}
                        </button>
                      </div>
                      <p className="text-[11px] text-t-3 leading-tight max-w-[260px] text-right">
                        请确认 AI 工具已停止运行，强行终止可能导致部分变更未被记录
                      </p>
                    </div>
                  );
                }

                return (
                  <button
                    onClick={() => setConfirmStop(true)}
                    className="flex items-center gap-2 px-4 py-1.5 rounded-md text-[13px] font-medium transition-colors bg-[rgba(0,0,0,0.06)] text-t-2 hover:bg-[rgba(0,0,0,0.1)] active:bg-[rgba(0,0,0,0.14)]"
                  >
                    <StopCircle className="w-[13px] h-[13px]" />
                    终止任务
                  </button>
                );
              }

              // Completed task with changes: show restore button
              if (totalCount > 0) {
                return (
                  <button
                    onClick={() => setShowRollback(true)}
                    className="flex items-center gap-2 px-4 py-1.5 bg-sys-blue text-white rounded-md text-[13px] font-medium hover:bg-sys-blue-hover transition-colors"
                  >
                    <RotateCcw className="w-[13px] h-[13px]" />
                    回到这一刻
                  </button>
                );
              }

              return null;
            })()}
          </div>
        </div>
      </div>

      {/* Scrollable content */}
      <div className="flex-1 overflow-y-auto min-h-0">
        {/* User prompt */}
        {!isWindow && task.prompt && (
          <div className="mx-5 mt-4 mb-3">
            <div className="text-[11px] font-semibold text-t-3 uppercase tracking-wider mb-2">
              Prompt
            </div>
            <div className="text-[13px] text-t-1 leading-relaxed">
              {task.prompt}
            </div>
          </div>
        )}

        {/* AI Summary */}
        {!isWindow && task.summary && task.summary !== task.prompt && (
          <div className="mx-5 mt-0 mb-3">
            <div className="text-[11px] font-semibold text-t-3 uppercase tracking-wider mb-2">
              AI Summary
            </div>
            <div className="text-[13px] text-t-1 leading-relaxed">
              {task.summary}
            </div>
          </div>
        )}

        {/* Task stats badges */}
        {!isWindow && (stats || displayDurationSecs != null) && (
          <div className="mx-5 mb-4 flex items-center gap-2 flex-wrap">
            {stats?.model && (
              <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-surface-hover text-[11px] text-t-2 font-medium">
                <svg className="w-3 h-3 text-t-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5"><circle cx="8" cy="8" r="5"/><path d="M6 8h4M8 6v4"/></svg>
                {stats.model}
              </span>
            )}
            {stats != null && stats.tool_calls > 0 && (
              <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-surface-hover text-[11px] text-t-2 tabular-nums">
                <svg className="w-3 h-3 text-t-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M4 12l3-3-3-3M9 12h4"/></svg>
                {stats.tool_calls} 次调用
              </span>
            )}
            {displayDurationSecs != null && (
              <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-surface-hover text-[11px] text-t-2 tabular-nums">
                <svg className="w-3 h-3 text-t-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><circle cx="8" cy="8" r="5.5"/><polyline points="8,5 8,8 10.5,9.5"/></svg>
                {formatDuration(displayDurationSecs)}
              </span>
            )}
          </div>
        )}

        {/* File stats summary */}
        <div className="mx-5 mt-3 mb-2 flex items-center gap-3 text-[12px]">
          <span className="text-[11px] font-semibold text-t-3 uppercase tracking-wider">
            Changed Files ({displayChangeCount})
          </span>
          <div className="flex items-center gap-3 ml-auto text-[11px] tabular-nums">
            {totalAdded > 0 && <span className="text-sys-green font-semibold">+{totalAdded}</span>}
            {totalRemoved > 0 && <span className="text-sys-red font-semibold">-{totalRemoved}</span>}
            <span className="text-t-3">{timeAgo(task.started_at)}</span>
          </div>
        </div>

        {restoreOperations.length > 0 && (
          <div className="mx-5 mb-2">
            <button
              onClick={() => setShowRestoreHistory(true)}
              className="flex items-center gap-1.5 text-[11px] text-t-3 hover:text-sys-blue transition-colors"
            >
              <Clock className="w-3 h-3" />
              恢复历史
              <span className="inline-flex items-center justify-center min-w-[16px] h-4 rounded-full bg-bg-grouped text-[10px] font-semibold text-t-2 px-1">
                {restoreOperations.length}
              </span>
            </button>
          </div>
        )}

        {visibleDeletedDirGroups.length > 0 && (
          <div className="mx-5 mb-2 space-y-1.5">
            {visibleDeletedDirGroups.map((group) => {
              const activeRestore = restoreProgress?.task_id === taskId && restoreProgress.dir_path === group.dir_path
                ? restoreProgress
                : null;
              return (
                <div key={group.dir_path} className="flex items-center justify-between gap-3 py-1.5">
                  <div className="flex items-center gap-2 min-w-0">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className="flex-shrink-0 text-sys-red">
                      <path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6l-.867 12.142A2 2 0 0 1 16.138 20H7.862a2 2 0 0 1-1.995-1.858L5 6" />
                    </svg>
                    <div className="min-w-0">
                      <span className="text-[12px] text-t-1 font-mono truncate block">{group.dir_path}</span>
                      <span className="text-[11px] text-t-3">
                        {onlyDeletedChangesLoaded
                          ? `整个目录被删除，共 ${group.total_files} 个文件`
                          : `${group.total_files} 个文件受影响`}
                      </span>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 flex-shrink-0" onClick={(e) => e.stopPropagation()}>
                    {confirmDirRestore === group.dir_path ? (
                      <>
                        <button
                          onClick={() => void handleDirectoryRestore(group.dir_path)}
                          disabled={dirRestoreBusy}
                          className="px-2 py-[3px] rounded text-[11px] font-medium text-sys-red hover:bg-sys-red/8 disabled:opacity-50 transition-colors"
                        >
                          {dirRestoreBusy ? restoreButtonLabel(activeRestore) : "确认恢复"}
                        </button>
                        <button
                          onClick={() => setConfirmDirRestore(null)}
                          disabled={dirRestoreBusy}
                          className="text-[11px] text-t-3 hover:text-t-1"
                        >
                          取消
                        </button>
                      </>
                    ) : (
                      <button
                        onClick={() => setConfirmDirRestore(group.dir_path)}
                        disabled={dirRestoreBusy}
                        className="px-2 py-[3px] rounded text-[11px] font-medium text-t-2 hover:text-t-1 hover:bg-bg-hover disabled:opacity-50 transition-colors"
                      >
                        恢复该目录
                      </button>
                    )}
                  </div>
                  {activeRestore && (
                    <div className="w-full mt-1">
                      <div className="h-[3px] w-full overflow-hidden rounded-full" style={{ background: "rgba(0,0,0,0.06)" }}>
                        <div
                          className="h-full rounded-full bg-sys-blue transition-all"
                          style={{
                            width: `${activeRestore.total_files > 0
                              ? Math.min(100, (activeRestore.processed_files / activeRestore.total_files) * 100)
                              : 0}%`,
                          }}
                        />
                      </div>
                      <div className="mt-1 text-[10px] text-t-3">
                        {restorePhaseLabel(activeRestore.phase)} · {activeRestore.processed_files}/{activeRestore.total_files}
                      </div>
                    </div>
                  )}
                  {dirRestoreError && confirmDirRestore === group.dir_path && (
                    <div className="w-full mt-1 text-[11px] text-sys-red">{dirRestoreError}</div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {truncated && (
          <div className="mx-5 mb-2 rounded-md border border-border bg-surface-hover px-3 py-2 text-[12px] text-t-2">
            该任务变更过大，当前仅展示前 {changes.length} 条样本，真实总数为 {totalCount} 条。
          </div>
        )}

        {/* File list with inline diff */}
        <div className="mx-5 mb-5">
          {changesLoading ? (
            <div className="py-4 text-t-3 text-[12px] text-center">加载中...</div>
          ) : changes.length === 0 ? (
            <div className="py-4 text-t-3 text-[12px] text-center">无文件变更</div>
          ) : (
            <div className="border border-border rounded-lg overflow-hidden">
              {changes.map((change) => (
                <FileRowWithDiff
                  key={change.id}
                  change={change}
                  taskId={taskId}
                  expanded={expandedFilePath === change.file_path}
                  onToggle={() => toggleFile(change.file_path)}
                  onRestored={() => {
                    refreshChanges();
                    listRestoreOperations(taskId).then(setRestoreOperations).catch(() => {});
                    getTask(taskId).then(setTask).catch(() => {});
                    onTaskUpdated();
                  }}
                />
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Rollback confirm modal */}
      {showRollback && (
        <RollbackPanel
          taskId={taskId}
          isMonitoringWindow={isWindow}
          windowLabel={isWindow ? windowLabel(task) : undefined}
          onClose={() => setShowRollback(false)}
          onRolledBack={() => {
            setShowRollback(false);
            onTaskUpdated();
            getTask(taskId).then(setTask);
            listRestoreOperations(taskId).then(setRestoreOperations).catch(() => {});
          }}
        />
      )}

      {/* Restore history modal */}
      {showRestoreHistory && (
        <RestoreHistoryModal
          operations={restoreOperations}
          onClose={() => setShowRestoreHistory(false)}
        />
      )}
    </div>
  );
}

// ─── File Row with inline expandable Diff ──────────────────────────

function FileRowWithDiff({
  change,
  taskId,
  expanded,
  onToggle,
  onRestored,
}: {
  change: ChangeInfo;
  taskId: string;
  expanded: boolean;
  onToggle: () => void;
  onRestored: () => void;
}) {
  const dotColor = DOT_COLOR[change.change_type] || "bg-sys-amber";
  const [restoring, setRestoring] = useState(false);
  const [justRestored, setJustRestored] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  const [confirmStep, setConfirmStep] = useState<"idle" | "confirming">("idle");

  // Diff data — loaded on expand
  const [diffText, setDiffText] = useState<string | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffLoaded, setDiffLoaded] = useState(false);
  const [imageBefore, setImageBefore] = useState<string | null>(null);
  const [imageAfter, setImageAfter] = useState<string | null>(null);
  const [imageMime, setImageMime] = useState<string>("image/png");

  // Detect if file is a previewable image by extension
  const imageExtensions: Record<string, string> = {
    png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg",
    gif: "image/gif", webp: "image/webp", bmp: "image/bmp", svg: "image/svg+xml",
  };
  const fileExt = change.file_path.split(".").pop()?.toLowerCase() ?? "";
  const isImage = fileExt in imageExtensions;

  // Load diff (and image previews for binary image files) when expanded
  useEffect(() => {
    if (expanded && !diffLoaded) {
      setDiffLoading(true);
      getChangeDiff(taskId, change.file_path)
        .then(async (res) => {
          setDiffText(res.diff_text);
          setDiffLoaded(true);
          if (!res.diff_text && isImage) {
            setImageMime(imageExtensions[fileExt] ?? "image/png");
            const [before, after] = await Promise.all([
              res.old_hash ? getObjectBase64(res.old_hash).catch(() => null) : Promise.resolve(null),
              res.new_hash ? getObjectBase64(res.new_hash).catch(() => null) : Promise.resolve(null),
            ]);
            setImageBefore(before);
            setImageAfter(after);
          }
        })
        .catch(() => { setDiffText(null); setDiffLoaded(true); })
        .finally(() => setDiffLoading(false));
    }
  }, [expanded, diffLoaded, taskId, change.file_path]); // eslint-disable-line react-hooks/exhaustive-deps

  const canRestore =
    (change.change_type === "modified" || change.change_type === "deleted" || change.change_type === "renamed") &&
    !!change.old_hash;

  const doRestore = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setConfirmStep("idle");
    setRestoring(true);
    setRestoreError(null);
    try {
      await restoreFile(taskId, change.file_path);
      setJustRestored(true);
      setTimeout(() => { onRestored(); setJustRestored(false); }, 2500);
    } catch (err) {
      setRestoreError(String(err));
      setTimeout(() => setRestoreError(null), 4000);
    } finally {
      setRestoring(false);
    }
  };

  const handleRestoreClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (confirmStep === "idle") {
      setConfirmStep("confirming");
      setTimeout(() => setConfirmStep("idle"), 4000);
    }
  };

  const handleCancel = (e: React.MouseEvent) => {
    e.stopPropagation();
    setConfirmStep("idle");
  };

  return (
    <div className={`border-b border-border-light last:border-b-0 ${expanded ? "bg-bg-grouped/30" : ""}`}>
      {/* File row — clickable to toggle diff */}
      <div
        onClick={onToggle}
        className="flex items-center gap-3 px-3.5 py-2.5 cursor-pointer transition-colors hover:bg-bg-hover"
      >
        {/* Expand chevron */}
        <span className="text-t-3 flex-shrink-0">
          {expanded ? <ChevronDown className="w-3.5 h-3.5" /> : <ChevronRight className="w-3.5 h-3.5" />}
        </span>

        {/* 7px color dot */}
        <span className={`w-[7px] h-[7px] rounded-full flex-shrink-0 ${dotColor}`} />

        <div className="flex-1 min-w-0">
          <div className={`text-[12px] font-mono truncate ${expanded ? "text-sys-blue font-medium" : "text-t-1"}`}>
            {change.change_type === "renamed" && change.old_file_path
              ? <>{fileName(change.old_file_path)} <span className="text-t-3">→</span> {fileName(change.file_path)}</>
              : fileName(change.file_path)
            }
          </div>
          <div className="text-[10px] text-t-3 truncate">
            {change.change_type === "renamed" && change.old_file_path
              ? <>{dirName(change.old_file_path)} <span className="text-t-4">→</span> {dirName(change.file_path)}</>
              : dirName(change.file_path)
            }
          </div>
        </div>

        <div className="flex items-center gap-1.5 flex-shrink-0">
          {change.lines_added > 0 && <span className="text-[11px] font-semibold text-sys-green tabular-nums">+{change.lines_added}</span>}
          {change.lines_removed > 0 && <span className="text-[11px] font-semibold text-sys-red tabular-nums">-{change.lines_removed}</span>}

          {/* Attribution confidence indicator */}
          {change.attribution && change.attribution !== "monitoring" && (
            <span
              className={`w-[6px] h-[6px] rounded-full flex-shrink-0 ${
                change.attribution === "hook"
                  ? "bg-sys-green"
                  : change.attribution === "bash_predicted"
                  ? "bg-sys-amber"
                  : "bg-t-4 opacity-50"
              }`}
              title={
                change.attribution === "hook" ? "精确归因 (Hook)"
                : change.attribution === "bash_predicted" ? "命令预测"
                : change.attribution === "fsevent_active" ? "FSEvent 活跃期"
                : change.attribution === "fsevent_grace" ? "FSEvent 缓冲期"
                : change.attribution
              }
            />
          )}

          {canRestore && (
            justRestored ? (
              <span className="text-[10px] text-sys-green font-medium">✓</span>
            ) : restoreError ? (
              <span className="text-[10px] text-sys-red">✕</span>
            ) : restoring ? (
              <span className="text-[10px] text-t-3 animate-spin">◐</span>
            ) : confirmStep === "confirming" ? (
              <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
                <button onClick={doRestore} className="text-[10px] text-sys-blue font-medium hover:underline">确认读档</button>
                <button onClick={handleCancel} className="text-[9px] text-t-4 hover:text-t-2">取消</button>
              </div>
            ) : (
              <button onClick={handleRestoreClick} className="text-[10px] text-t-4 hover:text-sys-blue transition-colors">
                读档
              </button>
            )
          )}
        </div>
      </div>

      {/* Inline diff — shown when expanded */}
      {expanded && (
        <div className="mx-3.5 mb-3 rounded-lg border border-zinc-800 overflow-hidden">
          {/* Diff file header */}
          <div className="flex items-center gap-2 px-4 py-1.5 bg-zinc-900 border-b border-zinc-800">
            <span className="text-[11px] font-mono text-zinc-400 truncate">
              {fileName(change.file_path)}
            </span>
            <span className="text-[10px] text-zinc-600 truncate ml-auto">
              {dirName(change.file_path)}
            </span>
          </div>
          {/* Diff content */}
          <div className="max-h-[400px] overflow-y-auto">
            <DiffViewer
              diffText={diffText}
              loading={diffLoading}
              imageBefore={imageBefore}
              imageAfter={imageAfter}
              imageMime={imageMime}
            />
          </div>
        </div>
      )}
    </div>
  );
}
