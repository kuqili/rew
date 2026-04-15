import { useState, useRef, useEffect, useCallback } from "react";
import { Search, ChevronDown } from "lucide-react";
import { useTasks } from "../hooks/useTasks";
import type { TaskInfo } from "../lib/tauri";
import { truncate } from "../lib/format";
import { getToolMeta } from "../lib/tools";
import type { ViewMode } from "./MainLayout";

/** Date filter modes */
type DateMode = "today" | "yesterday" | "24h" | "7d" | "custom";
interface DateFilter {
  mode: DateMode;
  date?: string;
}

interface Props {
  selectedId: string | null;
  onSelect: (id: string) => void;
  dirFilter?: string | null;
  viewMode: ViewMode;
  toolFilter: string | null;
}

function isMonitoringWindow(task: TaskInfo): boolean {
  return task.tool === "文件监听";
}

function effectiveTs(task: TaskInfo): string {
  return isMonitoringWindow(task)
    ? (task.completed_at ?? task.started_at)
    : task.started_at;
}

function toLocalDateStr(d: Date) {
  return d.toLocaleDateString("sv");
}
function todayStr() { return toLocalDateStr(new Date()); }
function yesterdayStr() {
  const d = new Date();
  d.setDate(d.getDate() - 1);
  return toLocalDateStr(d);
}

function inDateRange(task: TaskInfo, filter: DateFilter): boolean {
  const ts = new Date(effectiveTs(task));
  const now = new Date();
  switch (filter.mode) {
    case "today": return toLocalDateStr(ts) === todayStr();
    case "yesterday": return toLocalDateStr(ts) === yesterdayStr();
    case "24h": return now.getTime() - ts.getTime() < 86_400_000;
    case "7d": return now.getTime() - ts.getTime() < 7 * 86_400_000;
    case "custom": return filter.date ? toLocalDateStr(ts) === filter.date : toLocalDateStr(ts) === todayStr();
  }
}

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

function fmtHHMM(d: Date): string {
  return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
}

function formatWindowTime(task: TaskInfo): string {
  const start = new Date(task.started_at);
  const end = new Date(task.completed_at ?? task.started_at);
  const startMin = fmtHHMM(start);
  const endMin = fmtHHMM(end);
  if (startMin === endMin) return fmtHHMM(end);
  return `${startMin} – ${endMin}`;
}

function fmtTime(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
}

// ─── Date Picker ──────────────────────────────────────────────────

function DateFilterPicker({ value, onChange }: { value: DateFilter; onChange: (f: DateFilter) => void }) {
  const [open, setOpen] = useState(false);
  const [viewYear, setViewYear] = useState(() => new Date().getFullYear());
  const [viewMonth, setViewMonth] = useState(() => new Date().getMonth());
  const ref = useRef<HTMLDivElement>(null);

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

  const firstDay = new Date(viewYear, viewMonth, 1);
  const lastDay = new Date(viewYear, viewMonth + 1, 0);
  const startOffset = (firstDay.getDay() + 6) % 7;
  const today = todayStr();
  const selectedDate =
    value.mode === "custom" ? (value.date ?? today) :
    value.mode === "today" ? today :
    value.mode === "yesterday" ? yesterdayStr() : null;

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
  const cells: (number | null)[] = [
    ...Array(startOffset).fill(null),
    ...Array.from({ length: lastDay.getDate() }, (_, i) => i + 1),
  ];
  while (cells.length % 7 !== 0) cells.push(null);

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen(v => !v)}
        className={`flex items-center gap-1 px-2.5 py-[3px] rounded text-[12px] transition-colors ${
          open
            ? "bg-sys-blue text-white"
            : "text-t-3 hover:bg-bg-hover bg-white border border-border"
        }`}
      >
        <span>{filterLabel(value)}</span>
        <ChevronDown className="w-3 h-3 opacity-50" />
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1.5 z-50 bg-white border border-border rounded-xl shadow-lg overflow-hidden w-[260px]">
          <div className="flex items-center justify-between px-4 pt-3 pb-2">
            <button onClick={prevMonth} className="w-6 h-6 flex items-center justify-center rounded-md hover:bg-bg-hover text-t-3 text-[12px]">‹</button>
            <span className="text-[13px] font-semibold text-t-1">{viewYear}年{MONTH_NAMES[viewMonth]}</span>
            <button onClick={nextMonth} className="w-6 h-6 flex items-center justify-center rounded-md hover:bg-bg-hover text-t-3 text-[12px]">›</button>
          </div>
          <div className="grid grid-cols-7 px-3 pb-1">
            {DAY_NAMES.map(d => (
              <div key={d} className="text-center text-[10px] text-t-4 py-0.5">{d}</div>
            ))}
          </div>
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
                    isSelected ? "bg-sys-blue text-white font-semibold"
                      : isToday ? "border border-sys-blue text-sys-blue font-medium"
                        : isFuture ? "text-t-4 cursor-not-allowed"
                          : "text-t-2 hover:bg-bg-hover"
                  }`}
                >
                  {day}
                </button>
              );
            })}
          </div>
          <div className="flex items-center gap-1 px-3 py-2.5 border-t border-border/60 bg-bg-grouped/40 flex-wrap">
            {(["today", "yesterday", "24h", "7d"] as DateMode[]).map((mode) => {
              const labels: Record<DateMode, string> = { today: "今天", yesterday: "昨天", "24h": "近24h", "7d": "近7天", custom: "" };
              const active = value.mode === mode;
              return (
                <button
                  key={mode}
                  onClick={() => selectQuick(mode)}
                  className={`px-2.5 py-0.5 rounded-md text-[11px] transition-colors ${
                    active ? "bg-sys-blue text-white" : "text-t-2 hover:bg-bg-hover bg-white border border-border-light"
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

// ─── Timeline ──────────────────────────────────────────────────────

export default function TaskTimeline({ selectedId, onSelect, dirFilter, viewMode, toolFilter }: Props) {
  const [dateFilter, setDateFilter] = useState<DateFilter>({ mode: "today" });
  const [searchQuery, setSearchQuery] = useState("");
  const { tasks: allTasks, loading, error } = useTasks(dirFilter);

  // Apply date filter
  const dateTasks = allTasks.filter((t) => inDateRange(t, dateFilter));

  // Apply view mode + tool filter
  const filteredTasks = dateTasks.filter((t) => {
    if (viewMode === "ai") {
      if (isMonitoringWindow(t)) return false;
      if (toolFilter) {
        const taskToolKey = t.tool?.toLowerCase().replace(/_/g, "-") ?? "";
        return taskToolKey === toolFilter || t.tool === toolFilter;
      }
      return true;
    }
    return true; // "all" shows everything
  });

  // Apply search filter
  const tasks = searchQuery.trim()
    ? filteredTasks.filter((t) => {
        const q = searchQuery.toLowerCase();
        return (
          (t.prompt && t.prompt.toLowerCase().includes(q)) ||
          (t.summary && t.summary.toLowerCase().includes(q)) ||
          (t.tool && t.tool.toLowerCase().includes(q))
        );
      })
    : filteredTasks;

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-t-3 text-[13px]">
        <span className="animate-spin mr-2">◐</span> 加载中...
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full p-6">
        <div className="bg-sys-red/10 text-sys-red text-[13px] px-4 py-3 rounded-xl max-w-[320px]">
          <div className="font-medium mb-1">加载失败</div>
          <div className="text-[11px] opacity-80 break-all">{error}</div>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      {/* Header — compact, matching v7: title + filter on same row */}
      <div className="flex-shrink-0 px-4 pt-3 pb-2 flex items-center justify-between">
        <h2 className="text-[16px] font-semibold text-t-1 tracking-[-0.01em]">
          {viewMode === "ai" ? "AI 任务历史" : "最近活动"}
        </h2>
        <DateFilterPicker value={dateFilter} onChange={setDateFilter} />
      </div>

      {/* Search bar */}
      <div className="px-4 pb-3 border-b border-border">
        <div className="flex items-center gap-1.5 h-[26px] px-2 bg-bg-grouped border border-border-light rounded text-[12px]">
          <Search className="w-3 h-3 text-t-3 flex-shrink-0" />
          <input
            type="text"
            placeholder="搜索存档记录..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="flex-1 bg-transparent border-none outline-none text-[12px] text-t-1 placeholder:text-t-3"
          />
        </div>
      </div>

      {/* Entries */}
      <div className="flex-1 overflow-y-auto">
        {tasks.length === 0 ? (
          <EmptyTimeline mode={viewMode} label={filterLabel(dateFilter)} />
        ) : (
          <div>
            {tasks.map((task) => (
              <TaskCard
                key={task.id}
                task={task}
                selected={task.id === selectedId}
                onClick={() => onSelect(task.id)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ─── Task Card ─────────────────────────────────────────────────────

function TaskCard({
  task,
  selected,
  onClick,
}: {
  task: TaskInfo;
  selected: boolean;
  onClick: () => void;
}) {
  const isWindow = isMonitoringWindow(task);
  const isRolledBack = task.status === "rolled-back";
  const isActive = task.status === "active";
  const isFinalizing = task.finalization_status === "pending" || task.finalization_status === "running";
  const isFinalizeFailed = task.finalization_status === "failed";
  const toolMeta = !isWindow ? getToolMeta(task.tool) : null;

  let description: string;
  if (isWindow) {
    description = `${formatWindowTime(task)} 存档`;
  } else {
    description = task.prompt ? truncate(task.prompt, 60) : task.summary || "未记录操作";
  }

  const toolLabel = isWindow ? "自动存档" : (toolMeta?.label ?? "AI");
  const taskToolKey = task.tool?.toLowerCase().replace(/_/g, "-") ?? "";
  const toolColorClass = isWindow
    ? "text-t-3"
    : taskToolKey.includes("cursor")
      ? "text-tool-cursor"
      : "text-tool-claude";

  const time = fmtTime(effectiveTs(task));

  return (
    <button
      onClick={onClick}
      className={`w-full text-left px-4 py-3 border-b border-border-light cursor-default transition-colors
        ${selected ? "bg-sys-blue text-white" : "hover:bg-bg-hover"}`}
    >
      {/* Top row: tool label + status + time */}
      <div className="flex items-center gap-1 mb-0.5">
        <span className={`text-[11px] font-semibold ${selected ? "text-white/85" : toolColorClass}`}>
          {toolLabel}
        </span>

        {isRolledBack && (
          <span className={`text-[10px] px-1.5 py-0 rounded ${selected ? "bg-white/20 text-white/85" : "bg-sys-red/10 text-sys-red"}`}>
            已读档
          </span>
        )}
        {isActive && (
          <span className={`text-[10px] px-1.5 py-0 rounded ${selected ? "bg-white/20 text-white/85" : "bg-sys-amber/10 text-sys-amber"}`}>
            进行中
          </span>
        )}
        {isFinalizing && (
          <span className={`text-[10px] px-1.5 py-0 rounded ${selected ? "bg-white/20 text-white/85" : "bg-sys-blue/10 text-sys-blue"}`}>
            整理中
          </span>
        )}
        {isFinalizeFailed && (
          <span className={`text-[10px] px-1.5 py-0 rounded ${selected ? "bg-white/20 text-white/85" : "bg-sys-red/10 text-sys-red"}`}>
            整理失败
          </span>
        )}

        <span className={`ml-auto text-[12px] tabular-nums ${selected ? "text-white/70" : "text-t-3"}`}>
          {time}
        </span>
      </div>

      {/* Description */}
      <div className={`text-[13px] font-medium leading-snug mb-1 line-clamp-2 ${selected ? "text-white/95" : "text-t-1"}`}>
        {description}
      </div>

      {/* File count + line stats */}
      <div className="flex items-center gap-1 text-[11px]">
        {task.changes_count > 0 ? (
          <>
            <span className={`${selected ? "text-white/60" : "text-t-3"}`}>
              <span className={`font-semibold tabular-nums ${selected ? "text-white/80" : "text-t-2"}`}>{task.changes_count}</span> 个文件
            </span>
            {task.total_lines_added > 0 && (
              <span className={`tabular-nums font-semibold ${selected ? "text-green-300" : "text-sys-green"}`}>
                +{task.total_lines_added}
              </span>
            )}
            {task.total_lines_removed > 0 && (
              <span className={`tabular-nums font-semibold ${selected ? "text-red-300" : "text-sys-red"}`}>
                -{task.total_lines_removed}
              </span>
            )}
          </>
        ) : (
          <span className={`${selected ? "text-white/40" : "text-t-4"}`}>暂无文件变更</span>
        )}
      </div>
    </button>
  );
}

// ─── Empty state ───────────────────────────────────────────────────

function EmptyTimeline({ mode, label }: { mode: ViewMode; label: string }) {
  const isAi = mode === "ai";
  return (
    <div className="flex flex-col items-center justify-center h-full text-center px-6 py-8">
      <div className="w-10 h-10 rounded-lg bg-bg-grouped flex items-center justify-center mb-3">
        {isAi ? (
          <svg className="w-5 h-5 text-t-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M3.5 4h9M3.5 8h6M3.5 12h7.5"/>
          </svg>
        ) : (
          <svg className="w-5 h-5 text-t-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
            <circle cx="8" cy="8" r="5.5"/><polyline points="8,5 8,8 10.5,9.5"/>
          </svg>
        )}
      </div>
      <div className="text-[13px] text-t-2 font-medium mb-2">
        {label} {isAi ? "暂无 AI 任务" : "暂无活动"}
      </div>
      <div className="text-[11px] text-t-3 leading-relaxed max-w-[240px]">
        {isAi
          ? "AI 工具（Cursor、Claude Code）操作完成后会自动记录在这里。"
          : "文件变更会自动打包存档，AI 任务也会在这里显示。"}
      </div>
    </div>
  );
}
