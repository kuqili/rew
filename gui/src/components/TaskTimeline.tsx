import { useState, useRef, useEffect, useCallback } from "react";
import { useTasks } from "../hooks/useTasks";
import type { TaskInfo } from "../lib/tauri";
import { truncate } from "../lib/format";

type ViewMode = "scheduled" | "ai";

/** Date filter modes */
type DateMode = "today" | "yesterday" | "24h" | "7d" | "custom";
interface DateFilter {
  mode: DateMode;
  /** YYYY-MM-DD — only used when mode === "custom" */
  date?: string;
}

interface Props {
  selectedId: string | null;
  onSelect: (id: string) => void;
  dirFilter?: string | null;
}

/** Returns true if this task is a file-monitoring window (not an AI task). */
function isMonitoringWindow(task: TaskInfo): boolean {
  return task.tool === "文件监听";
}

/** The effective timestamp for a task. */
function effectiveTs(task: TaskInfo): string {
  return isMonitoringWindow(task)
    ? (task.completed_at ?? task.started_at)
    : task.started_at;
}

/** Format as YYYY-MM-DD HH:mm in local time. */
function fmtAbsTime(iso: string): string {
  const d = new Date(iso);
  const Y = d.getFullYear();
  const M = String(d.getMonth() + 1).padStart(2, "0");
  const D = String(d.getDate()).padStart(2, "0");
  const h = String(d.getHours()).padStart(2, "0");
  const m = String(d.getMinutes()).padStart(2, "0");
  return `${Y}-${M}-${D} ${h}:${m}`;
}

function toLocalDateStr(d: Date) {
  return d.toLocaleDateString("sv"); // YYYY-MM-DD in local tz
}

function todayStr() {
  return toLocalDateStr(new Date());
}

function yesterdayStr() {
  const d = new Date();
  d.setDate(d.getDate() - 1);
  return toLocalDateStr(d);
}

/** Returns true if the task falls within the selected date range. */
function inDateRange(task: TaskInfo, filter: DateFilter): boolean {
  const ts = new Date(effectiveTs(task));
  const now = new Date();
  const dayOf = toLocalDateStr;

  switch (filter.mode) {
    case "today":
      return dayOf(ts) === todayStr();
    case "yesterday":
      return dayOf(ts) === yesterdayStr();
    case "24h":
      return now.getTime() - ts.getTime() < 86_400_000;
    case "7d":
      return now.getTime() - ts.getTime() < 7 * 86_400_000;
    case "custom":
      return filter.date ? dayOf(ts) === filter.date : dayOf(ts) === todayStr();
  }
}

/** Human-readable label for the current filter. */
function filterLabel(filter: DateFilter): string {
  switch (filter.mode) {
    case "today": return "今天";
    case "yesterday": return "昨天";
    case "24h": return "近 24h";
    case "7d": return "近 7 天";
    case "custom": {
      if (!filter.date) return "选日期";
      const [, m, d] = filter.date.split("-");
      return `${parseInt(m)}月${parseInt(d)}日`;
    }
  }
}

/** Monitoring window: show only the completed_at time (seal moment). */
function formatWindowTime(task: TaskInfo): string {
  const ts = task.completed_at ?? task.started_at;
  return new Date(ts).toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

// ─── Date Picker ─────────────────────────────────────────────────────────────

interface DatePickerProps {
  value: DateFilter;
  onChange: (f: DateFilter) => void;
}

function DateFilterPicker({ value, onChange }: DatePickerProps) {
  const [open, setOpen] = useState(false);
  const [viewYear, setViewYear] = useState(() => new Date().getFullYear());
  const [viewMonth, setViewMonth] = useState(() => new Date().getMonth()); // 0-based
  const ref = useRef<HTMLDivElement>(null);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  const selectQuick = useCallback((mode: DateMode) => {
    onChange({ mode });
    setOpen(false);
  }, [onChange]);

  const selectDate = useCallback((dateStr: string) => {
    onChange({ mode: "custom", date: dateStr });
    setOpen(false);
  }, [onChange]);

  // Build calendar days
  const firstDay = new Date(viewYear, viewMonth, 1);
  const lastDay = new Date(viewYear, viewMonth + 1, 0);
  // Monday-based grid (0=Mon … 6=Sun)
  const startOffset = (firstDay.getDay() + 6) % 7; // Mon=0
  const today = todayStr();
  const selectedDate =
    value.mode === "custom" ? (value.date ?? today) :
    value.mode === "today" ? today :
    value.mode === "yesterday" ? yesterdayStr() :
    null;

  const prevMonth = () => {
    if (viewMonth === 0) { setViewYear(y => y - 1); setViewMonth(11); }
    else setViewMonth(m => m - 1);
  };
  const nextMonth = () => {
    if (viewMonth === 11) { setViewYear(y => y + 1); setViewMonth(0); }
    else setViewMonth(m => m + 1);
  };

  const MONTH_NAMES = ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];
  const DAY_NAMES = ["一", "二", "三", "四", "五", "六", "日"];

  // Grid cells: leading empty + day cells + trailing empty to fill 6 rows
  const cells: (number | null)[] = [
    ...Array(startOffset).fill(null),
    ...Array.from({ length: lastDay.getDate() }, (_, i) => i + 1),
  ];
  while (cells.length % 7 !== 0) cells.push(null);

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen(v => !v)}
        className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-[12px] border transition-colors ${
          open
            ? "bg-st-blue text-white border-st-blue"
            : "text-ink-secondary border-surface-border hover:border-st-blue/50 hover:text-st-blue bg-white"
        }`}
      >
        <span className="opacity-50 text-[11px]">时间筛选</span>
        <span className="opacity-30 mx-0.5">·</span>
        <span>{filterLabel(value)}</span>
        <span className="text-[9px] opacity-60">▾</span>
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 bg-white border border-surface-border rounded-xl shadow-lg overflow-hidden w-[260px]">

          {/* Month navigation */}
          <div className="flex items-center justify-between px-4 pt-3 pb-2">
            <button
              onClick={prevMonth}
              className="w-6 h-6 flex items-center justify-center rounded hover:bg-surface-hover text-ink-muted text-[12px]"
            >
              ‹
            </button>
            <span className="text-[13px] font-semibold text-ink">
              {viewYear}年{MONTH_NAMES[viewMonth]}
            </span>
            <button
              onClick={nextMonth}
              className="w-6 h-6 flex items-center justify-center rounded hover:bg-surface-hover text-ink-muted text-[12px]"
            >
              ›
            </button>
          </div>

          {/* Day-of-week header */}
          <div className="grid grid-cols-7 px-3 pb-1">
            {DAY_NAMES.map(d => (
              <div key={d} className="text-center text-[10px] text-ink-faint py-0.5">{d}</div>
            ))}
          </div>

          {/* Calendar grid */}
          <div className="grid grid-cols-7 px-3 pb-2">
            {cells.map((day, i) => {
              if (day === null) return <div key={i} />;
              const dateStr = `${viewYear}-${String(viewMonth + 1).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
              const isToday = dateStr === today;
              const isSelected = dateStr === selectedDate;
              const isFuture = dateStr > today;
              return (
                <button
                  key={i}
                  disabled={isFuture}
                  onClick={() => selectDate(dateStr)}
                  className={`aspect-square flex items-center justify-center rounded-full text-[12px] transition-colors mx-auto w-7 h-7 ${
                    isSelected
                      ? "bg-st-blue text-white font-semibold"
                      : isToday
                        ? "border border-st-blue text-st-blue font-medium"
                        : isFuture
                          ? "text-ink-faint cursor-not-allowed"
                          : "text-ink-secondary hover:bg-surface-hover"
                  }`}
                >
                  {day}
                </button>
              );
            })}
          </div>

          {/* Quick shortcuts */}
          <div className="flex items-center gap-1 px-3 py-2.5 border-t border-surface-border/60 bg-surface-secondary/40 flex-wrap">
            {(["today", "yesterday", "24h", "7d"] as DateMode[]).map((mode) => {
              const labels: Record<DateMode, string> = {
                today: "今天", yesterday: "昨天", "24h": "近24h", "7d": "近7天", custom: ""
              };
              const active = value.mode === mode;
              return (
                <button
                  key={mode}
                  onClick={() => selectQuick(mode)}
                  className={`px-2 py-0.5 rounded text-[11px] transition-colors border ${
                    active
                      ? "bg-st-blue text-white border-st-blue"
                      : "text-ink-secondary border-surface-border hover:border-st-blue/50 hover:text-st-blue bg-white"
                  }`}
                >
                  {labels[mode]}
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

// ─── Timeline ─────────────────────────────────────────────────────────────────

export default function TaskTimeline({ selectedId, onSelect, dirFilter }: Props) {
  const [viewMode, setViewMode] = useState<ViewMode>("scheduled");
  const [dateFilter, setDateFilter] = useState<DateFilter>({ mode: "today" });
  const { tasks: allTasks, loading, error } = useTasks(dirFilter);

  // Apply date filter first, then view mode
  const dateTasks = allTasks.filter((t) => inDateRange(t, dateFilter));
  const tasks = dateTasks.filter((t) =>
    viewMode === "scheduled" ? isMonitoringWindow(t) : !isMonitoringWindow(t),
  );

  const scheduledCount = dateTasks.filter(isMonitoringWindow).length;
  const aiCount = dateTasks.filter((t) => !isMonitoringWindow(t)).length;

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

  return (
    <div className="h-full flex flex-col">
      {/* Tab bar */}
      <div className="flex-shrink-0 border-b border-surface-border">
        <div className="flex items-center px-3 pt-1 gap-0">
          <button
            onClick={() => setViewMode("scheduled")}
            title="按设定时间间隔自动生成的存档点。AI 任务期间的文件变更不计入此视图，避免重复。"
            className={`px-3 py-1.5 text-[12px] border-b-2 transition-colors ${
              viewMode === "scheduled"
                ? "border-st-blue text-st-blue font-medium"
                : "border-transparent text-ink-muted hover:text-ink-secondary"
            }`}
          >
            定时存档{scheduledCount > 0 ? ` (${scheduledCount})` : ""}
          </button>
          <button
            onClick={() => setViewMode("ai")}
            title="由 AI 工具（Cursor、Claude Code 等）触发的存档，每次 AI 任务完成后自动生成一条记录。"
            className={`px-3 py-1.5 text-[12px] border-b-2 transition-colors ${
              viewMode === "ai"
                ? "border-st-blue text-st-blue font-medium"
                : "border-transparent text-ink-muted hover:text-ink-secondary"
            }`}
          >
            AI 任务{aiCount > 0 ? ` (${aiCount})` : ""}
          </button>

          <div className="flex-1" />

          {/* Date picker */}
          <div className="pb-1">
            <DateFilterPicker value={dateFilter} onChange={setDateFilter} />
          </div>
        </div>

        {/* Column headers */}
        <div className="table-header px-0">
          <div className="w-[60px] text-center px-2">图表</div>
          <div className="flex-1 px-3">描述</div>
          <div className="w-[100px] px-3 text-right">文件</div>
          <div className="w-[140px] px-3 text-right">时间</div>
        </div>
      </div>

      {/* Task rows */}
      <div className="flex-1 overflow-y-auto">
        {tasks.length === 0 ? (
          <EmptyTimeline mode={viewMode} filterLabel={filterLabel(dateFilter)} />
        ) : (
          tasks.map((task, index) => (
            <TaskRow
              key={task.id}
              task={task}
              isFirst={index === 0}
              isLast={index === tasks.length - 1}
              selected={task.id === selectedId}
              onClick={() => onSelect(task.id)}
            />
          ))
        )}
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
  const isWindow = isMonitoringWindow(task);
  const isRolledBack = task.status === "rolled-back";
  const isActive = task.status === "active";

  let description: string;
  if (isWindow) {
    description = `${formatWindowTime(task)} 存档`;
  } else {
    description = task.prompt ? truncate(task.prompt, 80) : task.summary || "未记录操作";
  }

  let toolLabel = "";
  if (!isWindow) {
    toolLabel = task.tool?.includes("claude")
      ? "Claude Code"
      : task.tool?.includes("cursor")
        ? "Cursor"
        : task.tool || "";
  }

  const dotClass = isRolledBack
    ? "w-[10px] h-[10px] rounded-full border-2 border-status-red bg-white"
    : isActive
      ? "w-[10px] h-[10px] rounded-full bg-status-yellow"
      : isWindow
        ? "w-[10px] h-[10px] rounded-full border-2 border-[#c8d1da] bg-white"
        : "w-[10px] h-[10px] rounded-full bg-st-blue";

  return (
    <button
      onClick={onClick}
      className={`task-row w-full text-left flex items-center border-b border-surface-border/60 ${
        selected ? "selected" : ""
      }`}
      style={{ minHeight: 36 }}
    >
      {/* Graph column */}
      <div className="w-[60px] flex justify-center relative" style={{ alignSelf: "stretch" }}>
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
        <div className="relative z-10 self-center">
          <div className={dotClass} />
        </div>
      </div>

      {/* Description column */}
      <div className="flex-1 py-2 px-1 min-w-0">
        <div className="flex items-center gap-2">
          {isRolledBack && (
            <span className="badge bg-status-red-bg text-status-red">已读档</span>
          )}
          {isActive && (
            <span className="badge bg-status-yellow-bg text-status-yellow">进行中</span>
          )}
          {toolLabel && (
            <span className="badge bg-st-blue-light text-st-blue">{toolLabel}</span>
          )}
          <span className={`text-[13px] truncate ${isWindow ? "text-ink-secondary font-mono" : "text-ink"}`}>
            {description}
          </span>
        </div>
      </div>

      {/* File count */}
      <div className="w-[100px] px-3 text-right text-[12px] text-ink-secondary tabular-nums flex-shrink-0">
        {task.changes_count > 0 && <span>{task.changes_count} 文件</span>}
      </div>

      {/* Time — absolute local timestamp, wider column for full date */}
      <div className="w-[140px] px-3 text-right text-[11px] text-ink-muted flex-shrink-0 tabular-nums">
        {fmtAbsTime(effectiveTs(task))}
      </div>
    </button>
  );
}

function EmptyTimeline({
  mode,
  filterLabel: label,
}: {
  mode: ViewMode;
  filterLabel: string;
}) {
  const isScheduled = mode === "scheduled";
  return (
    <div className="flex flex-col items-center justify-center h-full text-ink-muted py-8 select-none">
      <div className="text-3xl mb-3 opacity-20">{isScheduled ? "🕐" : "🤖"}</div>
      <div className="text-[13px] text-ink-muted font-medium mb-1">
        {label}{isScheduled ? "暂无存档" : "暂无 AI 任务"}
      </div>
      <div className="text-[11px] text-ink-faint text-center max-w-[240px] leading-relaxed">
        {isScheduled
          ? "rew 正在后台监控文件。按设定的时间间隔自动生成存档，存档出现后将显示在这里。"
          : "当 Cursor 或 Claude Code 完成任务后，此处会自动出现操作记录。"}
      </div>
    </div>
  );
}
