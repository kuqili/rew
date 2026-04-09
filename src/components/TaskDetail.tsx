import { useState, useEffect } from "react";
import { useTaskChanges } from "../hooks/useTasks";
import { type TaskInfo, getTask } from "../lib/tauri";
import { timeAgo, formatDateTime, fileName, dirName } from "../lib/format";
import DiffViewer from "./DiffViewer";
import UndoConfirm from "./UndoConfirm";
import type { ChangeInfo } from "../lib/tauri";

interface Props {
  taskId: string;
  onTaskUpdated: () => void;
  onBack: () => void;
}

export default function TaskDetail({ taskId, onTaskUpdated, onBack }: Props) {
  const [task, setTask] = useState<TaskInfo | null>(null);
  const { changes, loading: changesLoading } = useTaskChanges(taskId);
  const [showUndo, setShowUndo] = useState(false);
  const [expandedFile, setExpandedFile] = useState<number | null>(null);

  useEffect(() => {
    getTask(taskId).then(setTask).catch(() => setTask(null));
  }, [taskId]);

  if (!task) {
    return <div className="p-6 text-ink-muted text-sm">加载中...</div>;
  }

  const isRolledBack = task.status === "rolled-back";
  const totalAdded = changes.reduce((s, c) => s + c.lines_added, 0);
  const totalRemoved = changes.reduce((s, c) => s + c.lines_removed, 0);

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Task info header */}
      <div className="flex-shrink-0 bg-surface-secondary border-b border-surface-border px-6 py-4">
        <div className="flex items-start justify-between">
          <div className="flex-1 min-w-0">
            {/* Badges row */}
            <div className="flex items-center gap-2 mb-2">
              {task.tool && (
                <span className="badge bg-st-blue-light text-st-blue">
                  {task.tool.includes("claude") ? "Claude Code" : task.tool}
                </span>
              )}
              {isRolledBack && (
                <span className="badge bg-status-red-bg text-status-red">已撤销</span>
              )}
              <span className="text-2xs text-ink-muted">
                {formatDateTime(task.started_at)} · {timeAgo(task.started_at)}
              </span>
            </div>

            {/* Prompt */}
            {task.prompt && (
              <div className="text-[14px] text-ink leading-snug mb-2">
                {task.prompt}
              </div>
            )}

            {/* Stats */}
            <div className="flex items-center gap-4 text-2xs text-ink-secondary">
              <span>{changes.length} 个文件变更</span>
              {totalAdded > 0 && <span className="text-status-green">+{totalAdded} 行</span>}
              {totalRemoved > 0 && <span className="text-status-red">-{totalRemoved} 行</span>}
            </div>
          </div>

          {/* Undo button */}
          {!isRolledBack && changes.length > 0 && (
            <button
              onClick={() => setShowUndo(true)}
              className="px-4 py-2 rounded-md bg-white border border-surface-border text-sm text-ink-secondary hover:bg-surface-hover hover:text-status-red transition-colors shadow-sm"
            >
              ↩ 撤销
            </button>
          )}
        </div>
      </div>

      {/* File changes table */}
      <div className="flex-1 overflow-y-auto">
        {/* Table header */}
        <div className="table-header sticky top-0 z-10 px-6">
          <div className="w-[24px]" />
          <div className="flex-1 px-2">文件路径</div>
          <div className="w-[80px] text-right px-2">增删</div>
        </div>

        {changesLoading ? (
          <div className="p-6 text-ink-muted text-sm">加载中...</div>
        ) : changes.length === 0 ? (
          <div className="p-6 text-ink-muted text-sm text-center">无文件变更记录</div>
        ) : (
          <div>
            {changes.map((change) => (
              <FileRow
                key={change.id}
                change={change}
                expanded={expandedFile === change.id}
                onToggle={() => setExpandedFile(expandedFile === change.id ? null : change.id)}
              />
            ))}
          </div>
        )}
      </div>

      {/* Undo modal */}
      {showUndo && (
        <UndoConfirm
          taskId={taskId}
          task={task}
          onClose={() => setShowUndo(false)}
          onUndone={() => {
            setShowUndo(false);
            onTaskUpdated();
            getTask(taskId).then(setTask);
          }}
        />
      )}
    </div>
  );
}

function FileRow({
  change,
  expanded,
  onToggle,
}: {
  change: ChangeInfo;
  expanded: boolean;
  onToggle: () => void;
}) {
  const iconConfig: Record<string, { letter: string; bg: string; fg: string }> = {
    created:  { letter: "A", bg: "bg-status-green-bg", fg: "text-status-green" },
    modified: { letter: "M", bg: "bg-status-yellow-bg", fg: "text-status-yellow" },
    deleted:  { letter: "D", bg: "bg-status-red-bg", fg: "text-status-red" },
    renamed:  { letter: "D", bg: "bg-status-red-bg", fg: "text-status-red" },  // macOS trash = rename, show as delete
  };

  const icon = iconConfig[change.change_type] || iconConfig.modified;

  return (
    <>
      <button
        onClick={onToggle}
        className="w-full text-left flex items-center px-6 py-1.5 hover:bg-surface-hover transition-colors border-b border-surface-border/40"
      >
        {/* Type icon */}
        <div className={`change-icon ${icon.bg} ${icon.fg}`}>
          {icon.letter}
        </div>

        {/* File path */}
        <div className="flex-1 px-2 min-w-0">
          <span className="text-[13px] font-mono text-ink truncate block">
            {fileName(change.file_path)}
          </span>
          <span className="text-2xs text-ink-muted truncate block">
            {dirName(change.file_path)}
          </span>
        </div>

        {/* Line stats */}
        <div className="w-[80px] text-right px-2 text-2xs tabular-nums flex-shrink-0">
          {change.lines_added > 0 && (
            <span className="text-status-green mr-2">+{change.lines_added}</span>
          )}
          {change.lines_removed > 0 && (
            <span className="text-status-red">-{change.lines_removed}</span>
          )}
        </div>

        {/* Expand arrow */}
        <span className="text-ink-faint text-xs ml-1 w-4 text-center">
          {expanded ? "▾" : "▸"}
        </span>
      </button>

      {/* Diff panel */}
      {expanded && (
        <div className="border-b border-surface-border bg-surface-secondary">
          <DiffViewer diffText={change.diff_text} />
        </div>
      )}
    </>
  );
}
