import { useState, useEffect, useRef, useCallback } from "react";
import { useTaskChanges } from "../hooks/useTasks";
import { type TaskInfo, type ChangeInfo, getTask, restoreFile, getChangeDiff } from "../lib/tauri";
import { timeAgo, formatDateTime, fileName, dirName } from "../lib/format";
import DiffViewer from "./DiffViewer";
import RollbackPanel from "./RollbackPanel";

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

const CHANGE_ICON: Record<string, { letter: string; bg: string; fg: string }> = {
  created:  { letter: "A", bg: "bg-status-green-bg",  fg: "text-status-green" },
  modified: { letter: "M", bg: "bg-status-yellow-bg", fg: "text-status-yellow" },
  deleted:  { letter: "D", bg: "bg-status-red-bg",    fg: "text-status-red" },
  renamed:  { letter: "R", bg: "bg-surface-tertiary", fg: "text-ink-secondary" },
};

// ──────────────────────────────────────────────────────────────────
export default function TaskDetail({ taskId, dirFilter, onTaskUpdated, onBack }: Props) {
  const [task, setTask] = useState<TaskInfo | null>(null);
  const { changes, loading: changesLoading, refresh: refreshChanges } = useTaskChanges(taskId, dirFilter);
  const [showRollback, setShowRollback] = useState(false);

  // Selected file drives the right-side diff pane
  const [selectedFilePath, setSelectedFilePath] = useState<string | null>(null);
  const [diffText, setDiffText] = useState<string | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);

  // Resizable left panel width (px)
  const [leftWidth, setLeftWidth] = useState(280);
  const bodyRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  useEffect(() => {
    getTask(taskId).then(setTask).catch(() => setTask(null));
  }, [taskId]);

  // Auto-select first file when list loads
  useEffect(() => {
    if (changes.length > 0 && selectedFilePath === null) {
      setSelectedFilePath(changes[0].file_path);
    }
  }, [changes, selectedFilePath]);

  // Fetch diff whenever selection changes
  useEffect(() => {
    if (!selectedFilePath) { setDiffText(null); return; }
    setDiffLoading(true);
    setDiffText(null);
    getChangeDiff(taskId, selectedFilePath)
      .then((res) => setDiffText(res.diff_text))
      .catch(() => setDiffText(null))
      .finally(() => setDiffLoading(false));
  }, [taskId, selectedFilePath]);

  // ── Horizontal drag-to-resize ────────────────────────────────
  const onDividerMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    const onMove = (me: MouseEvent) => {
      if (!dragging.current || !bodyRef.current) return;
      const rect = bodyRef.current.getBoundingClientRect();
      const newW = Math.max(160, Math.min(me.clientX - rect.left, rect.width - 200));
      setLeftWidth(newW);
    };
    const onUp = () => {
      dragging.current = false;
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }, []);

  if (!task) return <div className="p-6 text-ink-muted text-sm">加载中...</div>;

  const isWindow = isMonitoringWindow(task);
  const isRolledBack = task.status === "rolled-back";
  const totalAdded = changes.reduce((s, c) => s + c.lines_added, 0);
  const totalRemoved = changes.reduce((s, c) => s + c.lines_removed, 0);

  const toolBadge = isWindow ? null
    : task.tool?.includes("claude") ? "Claude Code"
    : task.tool?.includes("cursor") ? "Cursor"
    : task.tool;

  const selectedChange = changes.find((c) => c.file_path === selectedFilePath) ?? null;

  return (
    <div className="h-full flex flex-col overflow-hidden">

      {/* ── Header: thin info bar ───────────────────────────────── */}
      <div className="flex-shrink-0 flex items-center gap-3 px-4 py-1.5 bg-surface-secondary border-b border-surface-border">
        {/* Badges */}
        {toolBadge && <span className="badge bg-st-blue-light text-st-blue flex-shrink-0">{toolBadge}</span>}
        {isWindow && (
          <span className="badge bg-surface-secondary text-ink-muted border border-surface-border/60 flex-shrink-0">
            文件监听
          </span>
        )}
        {isRolledBack && <span className="badge bg-status-red-bg text-status-red flex-shrink-0">已读档</span>}

        {/* Title / window range */}
        <span className={`text-[12px] truncate flex-1 min-w-0 ${isWindow ? "font-mono text-ink-secondary" : "text-ink"}`}>
          {isWindow ? windowLabel(task) : task.prompt || task.summary || "未记录操作"}
        </span>

        {/* Stats */}
        <div className="flex items-center gap-2 flex-shrink-0 text-[11px]">
          <span className="text-ink-secondary">{changes.length} 个文件</span>
          {totalAdded > 0 && <span className="text-status-green tabular-nums">+{totalAdded}</span>}
          {totalRemoved > 0 && <span className="text-status-red tabular-nums">-{totalRemoved}</span>}
          <span className="text-ink-muted">{timeAgo(task.started_at)}</span>
        </div>

        {/* Rollback button */}
        {changes.length > 0 && !showRollback && (
          <button
            onClick={() => setShowRollback(true)}
            title={isRolledBack
              ? "已读过档，可随时再次读档 — 操作不限次数，类似游戏存档点"
              : "读档：将此存档点内涉及的所有文件回到该存档点记录之前的版本"}
            className={[
              "flex-shrink-0 px-2.5 py-1 rounded border text-[11px] transition-colors bg-white",
              isRolledBack
                ? "border-surface-border text-ink-faint hover:border-st-blue hover:text-st-blue"
                : "border-surface-border text-ink-secondary hover:border-status-red hover:text-status-red",
            ].join(" ")}
          >
            {isRolledBack ? "↩ 再次读档" : "↩ 读档"}
          </button>
        )}
      </div>

      {/* ── Rollback panel ──────────────────────────────────────── */}
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

      {/* ── SourceTree-style: left file list | right diff ───────── */}
      <div ref={bodyRef} className="flex-1 flex flex-row overflow-hidden min-h-0">

        {/* ── Left: file list ─────────────────────────────────────── */}
        <div
          className="flex-shrink-0 flex flex-col overflow-hidden border-r border-surface-border bg-surface-secondary"
          style={{ width: leftWidth }}
        >
          {/* Column header */}
          <div className="flex-shrink-0 flex items-center px-3 py-1.5 border-b border-surface-border/60 bg-surface-tertiary">
            <span className="text-[10px] text-ink-faint font-medium uppercase tracking-wide select-none">
              变更文件 {changes.length > 0 && `(${changes.length})`}
            </span>
          </div>

          {/* File rows */}
          <div className="flex-1 overflow-y-auto">
            {changesLoading ? (
              <div className="p-4 text-ink-muted text-xs">加载中...</div>
            ) : changes.length === 0 ? (
              <div className="p-4 text-ink-muted text-xs text-center">无文件变更</div>
            ) : (
              changes.map((change) => (
                <FileListRow
                  key={change.id}
                  change={change}
                  selected={change.file_path === selectedFilePath}
                  taskId={taskId}
                  onSelect={() => setSelectedFilePath(change.file_path)}
                  onRestored={refreshChanges}
                />
              ))
            )}
          </div>
        </div>

        {/* ── Drag divider ─────────────────────────────────────────── */}
        <div
          onMouseDown={onDividerMouseDown}
          className="flex-shrink-0 w-[4px] bg-surface-border/30 hover:bg-st-blue/40 cursor-col-resize transition-colors flex items-center justify-center group"
          title="拖拽调整宽度"
        />

        {/* ── Right: diff pane ─────────────────────────────────────── */}
        <div className="flex-1 flex flex-col overflow-hidden min-h-0 bg-white">
          {/* Diff pane header: current file */}
          <div className="flex-shrink-0 flex items-center gap-2 px-4 py-1.5 border-b border-surface-border bg-surface-secondary">
            {selectedChange ? (
              <>
                {(() => {
                  const icon = CHANGE_ICON[selectedChange.change_type] ?? CHANGE_ICON.modified;
                  return (
                    <span className={`change-icon text-[10px] w-4 h-4 flex-shrink-0 ${icon.bg} ${icon.fg}`}>
                      {icon.letter}
                    </span>
                  );
                })()}
                <span className="text-xs font-mono text-ink truncate flex-1">
                  {fileName(selectedChange.file_path)}
                </span>
                <span className="text-[10px] text-ink-muted truncate hidden lg:block max-w-[240px]">
                  {dirName(selectedChange.file_path)}
                </span>
                <div className="flex items-center gap-2 flex-shrink-0 ml-2">
                  {selectedChange.lines_added > 0 && (
                    <span className="text-[11px] text-status-green tabular-nums">
                      +{selectedChange.lines_added}
                    </span>
                  )}
                  {selectedChange.lines_removed > 0 && (
                    <span className="text-[11px] text-status-red tabular-nums">
                      -{selectedChange.lines_removed}
                    </span>
                  )}
                </div>
              </>
            ) : (
              <span className="text-[11px] text-ink-faint">点击左侧文件查看 diff</span>
            )}
          </div>

          {/* Diff content */}
          <div className="flex-1 overflow-auto min-h-0">
            <DiffViewer diffText={diffText} loading={diffLoading} />
          </div>
        </div>

      </div>
    </div>
  );
}

// ──────────────────────────────────────────────────────────────────
// FileListRow
// ──────────────────────────────────────────────────────────────────
function FileListRow({
  change,
  selected,
  taskId,
  onSelect,
  onRestored,
}: {
  change: ChangeInfo;
  selected: boolean;
  taskId: string;
  onSelect: () => void;
  onRestored: () => void;
}) {
  const icon = CHANGE_ICON[change.change_type] ?? CHANGE_ICON.modified;
  const [restoring, setRestoring] = useState(false);
  const [justRestored, setJustRestored] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  // Two-step confirmation: null = idle, "confirming" = waiting for 2nd click
  const [confirmStep, setConfirmStep] = useState<"idle" | "confirming">("idle");

  const lastRestoredHint = change.restored_at
    ? `上次读档: ${timeAgo(change.restored_at)}`
    : null;

  const canRestore =
    (change.change_type === "modified" || change.change_type === "deleted") &&
    !!change.old_hash;

  const doRestore = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setConfirmStep("idle");
    setRestoring(true);
    setRestoreError(null);
    try {
      await restoreFile(taskId, change.file_path);
      setJustRestored(true);
      setTimeout(() => {
        onRestored();
        setJustRestored(false);
      }, 2500);
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
      // Auto-cancel after 4 s if user does nothing
      setTimeout(() => setConfirmStep("idle"), 4000);
    }
  };

  const handleCancel = (e: React.MouseEvent) => {
    e.stopPropagation();
    setConfirmStep("idle");
  };

  return (
    <div
      onClick={onSelect}
      className={[
        "flex flex-col px-3 py-2 cursor-pointer border-b border-surface-border/30 transition-colors select-none",
        selected
          ? "bg-st-blue/10 border-l-2 border-l-st-blue"
          : "hover:bg-surface-hover border-l-2 border-l-transparent",
      ].join(" ")}
    >
      {/* Row top: icon + filename */}
      <div className="flex items-center gap-2 min-w-0">
        <span className={`change-icon text-[9px] w-4 h-4 flex-shrink-0 ${icon.bg} ${icon.fg}`}>
          {icon.letter}
        </span>
        <span className={`text-[12px] font-mono truncate leading-tight ${selected ? "text-st-blue font-medium" : "text-ink"}`}>
          {fileName(change.file_path)}
        </span>
      </div>

      {/* Row bottom: dir + stats + restore action */}
      <div className="flex items-center justify-between mt-0.5 pl-6 min-w-0">
        <span className="text-[10px] text-ink-muted truncate flex-1 leading-none">
          {dirName(change.file_path)}
        </span>

        <div className="flex items-center gap-1.5 flex-shrink-0 ml-2">
          {change.lines_added > 0 && (
            <span className="text-[10px] text-status-green tabular-nums">+{change.lines_added}</span>
          )}
          {change.lines_removed > 0 && (
            <span className="text-[10px] text-status-red tabular-nums">-{change.lines_removed}</span>
          )}

          {canRestore && (
            justRestored ? (
              <span className="text-[9px] text-status-green font-medium">✓ 已读档</span>
            ) : restoreError ? (
              <span className="text-[9px] text-status-red" title={restoreError}>失败，点击重试</span>
            ) : restoring ? (
              <span className="text-[9px] text-ink-muted">读档中…</span>
            ) : confirmStep === "confirming" ? (
              /* Step 2: confirm / cancel */
              <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
                <span className="text-[9px] text-ink-secondary">覆盖当前版本？</span>
                <button
                  onClick={doRestore}
                  className="text-[9px] bg-status-red text-white rounded px-1.5 py-0.5 leading-none hover:opacity-90 transition-opacity"
                >
                  确认
                </button>
                <button
                  onClick={handleCancel}
                  className="text-[9px] text-ink-faint hover:text-ink-secondary leading-none"
                >
                  取消
                </button>
              </div>
            ) : (
              /* Step 1: first click */
              <button
                onClick={handleRestoreClick}
                title={[
                  "将此文件回到该存档点之前的版本。",
                  change.change_type === "deleted" ? "（此文件在该时间段内被删除，读档将恢复它）" : "",
                  lastRestoredHint ?? "",
                ].filter(Boolean).join("\n")}
                className="text-[9px] text-ink-faint border border-surface-border/50 rounded px-1.5 py-0.5 hover:text-st-blue hover:border-st-blue transition-colors leading-none"
              >
                ↩ 读档
              </button>
            )
          )}
        </div>
      </div>
    </div>
  );
}
