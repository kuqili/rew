import { useState, useEffect } from "react";
import { RotateCcw, ChevronDown, ChevronRight } from "lucide-react";
import { useTaskChanges } from "../hooks/useTasks";
import { type TaskInfo, type ChangeInfo, type TaskStatsInfo, getTask, getTaskStats, restoreFile, getChangeDiff } from "../lib/tauri";
import { timeAgo, fileName, dirName } from "../lib/format";
import { getToolMeta } from "../lib/tools";
import DiffViewer from "./DiffViewer";
import RollbackPanel from "./RollbackPanel";

// Format duration: e.g. 90s → "1m30s", 60s → "1m"
function formatDuration(secs: number): string {
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  const s = Math.round(secs % 60);
  return s > 0 ? `${m}m${s}s` : `${m}m`;
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
  const { changes, loading: changesLoading, refresh: refreshChanges } = useTaskChanges(taskId, dirFilter);
  const [showRollback, setShowRollback] = useState(false);

  // Which file is expanded to show diff (null = none)
  const [expandedFilePath, setExpandedFilePath] = useState<string | null>(null);

  useEffect(() => {
    getTask(taskId).then(setTask).catch(() => setTask(null));
    getTaskStats(taskId).then(setStats).catch(() => setStats(null));
  }, [taskId]);

  // Reset expanded file when task changes
  useEffect(() => {
    setExpandedFilePath(null);
  }, [taskId]);

  if (!task) {
    return (
      <div className="h-full flex items-center justify-center text-t-3 text-[13px]">
        <span className="animate-spin mr-2">◐</span> 加载中...
      </div>
    );
  }

  const isWindow = isMonitoringWindow(task);
  const isRolledBack = task.status === "rolled-back";
  const totalAdded = changes.reduce((s, c) => s + c.lines_added, 0);
  const totalRemoved = changes.reduce((s, c) => s + c.lines_removed, 0);

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
          </div>

          {/* Restore button */}
          {changes.length > 0 && !showRollback && (
            <button
              onClick={() => setShowRollback(true)}
              className="flex items-center gap-2 px-4 py-1.5 bg-sys-blue text-white rounded-md text-[13px] font-medium hover:bg-sys-blue-hover transition-colors"
            >
              <RotateCcw className="w-[13px] h-[13px]" />
              回到这一刻
            </button>
          )}
        </div>
      </div>

      {/* Rollback panel (inline) */}
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
          }}
        />
      )}

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
            Changed Files ({changes.length})
          </span>
          <div className="flex items-center gap-3 ml-auto text-[11px] tabular-nums">
            {totalAdded > 0 && <span className="text-sys-green font-semibold">+{totalAdded}</span>}
            {totalRemoved > 0 && <span className="text-sys-red font-semibold">-{totalRemoved}</span>}
            <span className="text-t-3">{timeAgo(task.started_at)}</span>
          </div>
        </div>

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
                  onRestored={refreshChanges}
                />
              ))}
            </div>
          )}
        </div>
      </div>
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

  // Load diff when expanded
  useEffect(() => {
    if (expanded && !diffLoaded) {
      setDiffLoading(true);
      getChangeDiff(taskId, change.file_path)
        .then((res) => { setDiffText(res.diff_text); setDiffLoaded(true); })
        .catch(() => { setDiffText(null); setDiffLoaded(true); })
        .finally(() => setDiffLoading(false));
    }
  }, [expanded, diffLoaded, taskId, change.file_path]);

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
            <DiffViewer diffText={diffText} loading={diffLoading} />
          </div>
        </div>
      )}
    </div>
  );
}
