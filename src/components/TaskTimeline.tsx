import { useTasks } from "../hooks/useTasks";
import type { TaskInfo } from "../lib/tauri";
import { timeAgo, truncate } from "../lib/format";

interface Props {
  selectedId: string | null;
  onSelect: (id: string) => void;
}

export default function TaskTimeline({ selectedId, onSelect }: Props) {
  const { tasks, loading, error } = useTasks();

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-ink-muted text-sm">
        加载中...
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="bg-status-red-bg text-status-red text-sm px-4 py-3 rounded-lg max-w-[400px]">
          <div className="font-medium mb-1">加载任务失败</div>
          <div className="text-xs opacity-80 break-all">{error}</div>
        </div>
      </div>
    );
  }

  if (tasks.length === 0) {
    return <EmptyTimeline />;
  }

  return (
    <div className="h-full flex flex-col">
      {/* Table header — Sourcetree style */}
      <div className="table-header px-0 flex-shrink-0">
        <div className="w-[60px] text-center px-2">图表</div>
        <div className="flex-1 px-3">描述</div>
        <div className="w-[100px] px-3 text-right">文件</div>
        <div className="w-[100px] px-3 text-right">时间</div>
      </div>

      {/* Task rows with timeline graph */}
      <div className="flex-1 overflow-y-auto">
        {tasks.map((task, index) => (
          <TaskRow
            key={task.id}
            task={task}
            isFirst={index === 0}
            isLast={index === tasks.length - 1}
            selected={task.id === selectedId}
            onClick={() => onSelect(task.id)}
          />
        ))}
      </div>
    </div>
  );
}

function TaskRow({
  task,
  isFirst,
  isLast,
  selected,
  onClick,
}: {
  task: TaskInfo;
  isFirst: boolean;
  isLast: boolean;
  selected: boolean;
  onClick: () => void;
}) {
  const description = task.prompt
    ? truncate(task.prompt, 80)
    : task.summary || "未记录操作";

  const statusBadge = task.status === "rolled-back"
    ? { text: "已撤销", cls: "bg-status-red-bg text-status-red" }
    : task.status === "active"
      ? { text: "进行中", cls: "bg-status-yellow-bg text-status-yellow" }
      : null;

  const toolLabel = task.tool?.includes("claude")
    ? "Claude Code"
    : task.tool?.includes("cursor")
      ? "Cursor"
      : task.tool || "";

  return (
    <button
      onClick={onClick}
      className={`task-row w-full text-left flex items-center border-b border-surface-border/60 ${
        selected ? "selected" : ""
      }`}
      style={{ minHeight: 36 }}
    >
      {/* Graph column — blue dot + vertical line */}
      <div className="w-[60px] flex justify-center relative" style={{ alignSelf: "stretch" }}>
        {/* Vertical line */}
        {!isFirst && (
          <div
            className="absolute left-1/2 top-0 w-[2px] bg-[#c8d1da]"
            style={{ transform: "translateX(-50%)", height: "50%" }}
          />
        )}
        {!isLast && (
          <div
            className="absolute left-1/2 bottom-0 w-[2px] bg-[#c8d1da]"
            style={{ transform: "translateX(-50%)", height: "50%" }}
          />
        )}
        {/* Blue dot */}
        <div className="relative z-10 self-center">
          <div
            className={`w-[10px] h-[10px] rounded-full ${
              task.status === "rolled-back"
                ? "bg-ink-faint"
                : task.status === "active"
                  ? "bg-status-yellow"
                  : "bg-st-blue"
            }`}
          />
        </div>
      </div>

      {/* Description column */}
      <div className="flex-1 py-2 px-1 min-w-0">
        <div className="flex items-center gap-2">
          {statusBadge && (
            <span className={`badge ${statusBadge.cls}`}>
              {statusBadge.text}
            </span>
          )}
          {toolLabel && (
            <span className="badge bg-st-blue-light text-st-blue">
              {toolLabel}
            </span>
          )}
          <span className="text-[13px] text-ink truncate">
            {description}
          </span>
        </div>
      </div>

      {/* File count column */}
      <div className="w-[100px] px-3 text-right text-[12px] text-ink-secondary tabular-nums flex-shrink-0">
        {task.changes_count > 0 && (
          <span>{task.changes_count} 文件</span>
        )}
      </div>

      {/* Time column */}
      <div className="w-[100px] px-3 text-right text-[12px] text-ink-muted flex-shrink-0">
        {timeAgo(task.started_at)}
      </div>
    </button>
  );
}

function EmptyTimeline() {
  return (
    <div className="flex flex-col items-center justify-center h-full text-ink-muted">
      <div className="text-4xl mb-4 opacity-40">🛡️</div>
      <div className="text-sm font-medium text-ink-secondary mb-1">
        等待 AI 操作
      </div>
      <div className="text-2xs text-ink-muted max-w-[260px] text-center leading-relaxed">
        使用 Claude Code 或 Cursor 时，每次操作都会自动记录在这里。
        <br />
        运行 <code className="font-mono bg-surface-secondary px-1 rounded">rew install</code> 注入 Hook。
      </div>
    </div>
  );
}
