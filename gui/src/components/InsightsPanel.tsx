import { useState, useMemo } from "react";
import { useInsights, type InsightPeriod, type ToolUsageStat, type DailyPoint, type TopTask } from "../hooks/useInsights";
import { getToolMeta } from "../lib/tools";
import { getToolBrandIcon } from "./ToolIcons";

// ─── Format Helpers ─────────────────────────────────────────────

function fmtDuration(secs: number): string {
  if (secs < 60) return `${Math.round(secs)}s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rm = m % 60;
  if (rm === 0) return `${h}h`;
  return `${h}h ${rm}m`;
}

function fmtDurationLong(secs: number): string {
  if (secs < 60) return `${Math.round(secs)} 秒`;
  const h = secs / 3600;
  if (h >= 1) return `${h.toFixed(1)}h`;
  return `${Math.round(secs / 60)}m`;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

function fmtCost(usd: number): string {
  if (usd < 0.01) return "$0.00";
  return `$${usd.toFixed(2)}`;
}

function periodLabel(period: InsightPeriod): string {
  switch (period) {
    case "day": return "今日";
    case "week": return "本周";
    case "month": return "本月";
  }
}

function periodDateRange(period: InsightPeriod): string {
  const now = new Date();
  const fmt = (d: Date) =>
    `${d.getFullYear()}.${String(d.getMonth() + 1).padStart(2, "0")}.${String(d.getDate()).padStart(2, "0")}`;
  const end = fmt(now);
  const start = new Date(now);
  switch (period) {
    case "day": return end;
    case "week": start.setDate(start.getDate() - 6); break;
    case "month": start.setDate(start.getDate() - 29); break;
  }
  return `${fmt(start)} – ${end}`;
}

// ─── Main Component ─────────────────────────────────────────────

export default function InsightsPanel() {
  const [period, setPeriod] = useState<InsightPeriod>("week");
  const { data, loading } = useInsights(period);

  if (loading && !data) {
    return (
      <div className="h-full flex items-center justify-center">
        <span className="text-t-3 text-[13px]">加载中…</span>
      </div>
    );
  }

  if (!data || data.totalTasks === 0) {
    return <EmptyInsights period={period} onPeriodChange={setPeriod} />;
  }

  return (
    <div className="h-full flex overflow-hidden">
      {/* Left: Summary sidebar */}
      <aside className="w-[280px] flex-shrink-0 border-r border-border bg-[#fbfbfb] flex flex-col overflow-y-auto">
        <SummarySidebar data={data} period={period} onPeriodChange={setPeriod} />
      </aside>

      {/* Right: Detail area */}
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden bg-white">
        <DetailArea data={data} period={period} />
      </main>
    </div>
  );
}

// ─── Empty State ────────────────────────────────────────────────

function EmptyInsights({
  period,
  onPeriodChange,
}: {
  period: InsightPeriod;
  onPeriodChange: (p: InsightPeriod) => void;
}) {
  return (
    <div className="h-full flex flex-col">
      <div className="px-6 pt-5 pb-4">
        <PeriodPicker period={period} onChange={onPeriodChange} />
      </div>
      <div className="flex-1 flex flex-col items-center justify-center select-none px-8">
        <div className="w-14 h-14 rounded-2xl bg-bg-grouped flex items-center justify-center mb-4">
          <svg className="w-7 h-7 text-t-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 013 19.875v-6.75zM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 01-1.125-1.125V8.625zM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 01-1.125-1.125V4.125z" />
          </svg>
        </div>
        <div className="text-[14px] text-t-2 font-medium mb-1.5">暂无 AI 任务数据</div>
        <div className="text-[12px] text-t-3 max-w-[260px] leading-relaxed text-center">
          {periodLabel(period)}还没有 AI 任务记录。使用 Claude Code、Cursor 等 AI 工具进行编码后，使用洞察将自动生成。
        </div>
      </div>
    </div>
  );
}

// ─── Period Picker (Segmented Control) ──────────────────────────

function PeriodPicker({
  period,
  onChange,
}: {
  period: InsightPeriod;
  onChange: (p: InsightPeriod) => void;
}) {
  const options: { value: InsightPeriod; label: string }[] = [
    { value: "day", label: "日" },
    { value: "week", label: "周" },
    { value: "month", label: "月" },
  ];

  return (
    <div className="flex bg-black/[0.04] p-0.5 rounded-lg w-full text-[11px] font-medium text-center">
      {options.map((opt) => (
        <button
          key={opt.value}
          onClick={() => onChange(opt.value)}
          className={`flex-1 py-1 rounded-md transition-all cursor-default
            ${period === opt.value
              ? "bg-white shadow-sm text-t-1"
              : "text-t-3 hover:text-t-2"
            }`}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

// ─── Left: Summary Sidebar ──────────────────────────────────────

function SummarySidebar({
  data,
  period,
  onPeriodChange,
}: {
  data: NonNullable<ReturnType<typeof useInsights>["data"]>;
  period: InsightPeriod;
  onPeriodChange: (p: InsightPeriod) => void;
}) {
  return (
    <div className="p-5 space-y-6">
      {/* Header */}
      <div>
        <h1 className="text-[14px] font-bold text-t-1 mb-4">使用摘要</h1>
        <PeriodPicker period={period} onChange={onPeriodChange} />
      </div>

      {/* Core metrics */}
      <GroupSection label="核心指标">
        <MetricRow label="Token 消耗" value="开发中" valueClass="text-t-4 text-[11px]" badge="coming" />
        <MetricRow label="协作时长" value={fmtDurationLong(data.totalDurationSecs)} />
        <MetricRow label="AI 任务数" value={`${data.totalTasks}`} />
        <MetricRow label="文件变更" value={`${data.totalFilesChanged}`} last />
      </GroupSection>

      {/* Estimated cost */}
      <GroupSection label="预估费用">
        <MetricRow
          label={`${periodLabel(period)}支出 (USD)`}
          value="开发中"
          valueClass="text-t-4 text-[11px]"
          badge="coming"
          last
        />
      </GroupSection>

      {/* Per-tool summary */}
      {data.toolStats.length > 0 && (
        <GroupSection label="工具分布">
          {data.toolStats.map((ts, i) => (
            <ToolSummaryRow
              key={ts.toolKey}
              stat={ts}
              last={i === data.toolStats.length - 1}
            />
          ))}
        </GroupSection>
      )}
    </div>
  );
}

// ─── Grouped Section ────────────────────────────────────────────

function GroupSection({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="px-1 text-[10px] font-semibold text-t-3 uppercase tracking-wider mb-2">
        {label}
      </div>
      <div className="bg-white border border-border rounded-lg overflow-hidden shadow-[0_0.5px_2px_rgba(0,0,0,0.04)]">
        {children}
      </div>
    </div>
  );
}

// ─── Metric Row ─────────────────────────────────────────────────

function MetricRow({
  label,
  value,
  valueClass = "",
  badge,
  last = false,
}: {
  label: string;
  value: string;
  valueClass?: string;
  badge?: "coming";
  last?: boolean;
}) {
  return (
    <div
      className={`flex items-center justify-between px-3 py-[7px]
        ${last ? "" : "border-b border-black/[0.04]"}`}
    >
      <span className="text-[12px] text-t-2 font-medium">{label}</span>
      <span className={`flex items-center gap-1.5 text-[15px] font-medium tabular-nums ${valueClass || "text-t-1"}`}>
        {badge === "coming" && (
          <span className="text-[9px] font-semibold text-t-4 bg-black/[0.04] px-1.5 py-[1px] rounded">开发中</span>
        )}
        {badge !== "coming" && value}
      </span>
    </div>
  );
}

// ─── Tool Summary Row ───────────────────────────────────────────

function ToolSummaryRow({
  stat,
  last,
}: {
  stat: ToolUsageStat;
  last: boolean;
}) {
  return (
    <div className={`px-3 py-[7px] ${last ? "" : "border-b border-black/[0.04]"}`}>
      <div className="flex items-center justify-between mb-1.5">
        <span className="flex items-center gap-1.5 text-[12px] font-medium text-t-1">
          {getToolBrandIcon(stat.toolKey, 14)}
          {stat.label}
        </span>
        <span className="text-[12px] font-mono text-t-3">{stat.durationPercent.toFixed(0)}%</span>
      </div>
      <div className="h-[3px] rounded-full bg-black/[0.04] overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{ width: `${Math.max(2, stat.durationPercent)}%`, backgroundColor: stat.color }}
        />
      </div>
    </div>
  );
}

// ─── Right: Detail Area ─────────────────────────────────────────

function DetailArea({
  data,
  period,
}: {
  data: NonNullable<ReturnType<typeof useInsights>["data"]>;
  period: InsightPeriod;
}) {
  return (
    <>
      {/* Header */}
      <header className="h-[44px] px-7 flex items-center justify-between border-b border-border flex-shrink-0">
        <span className="text-[13px] font-bold text-t-1">效能报告</span>
        <span className="text-[10px] text-t-3 font-mono">{periodDateRange(period)}</span>
      </header>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-8 pb-12">
        {/* Trend chart */}
        {data.dailyPoints.length > 1 && (
          <div className="mb-10">
            <div className="text-[10px] font-semibold text-t-3 uppercase tracking-wider mb-5">
              协作时长趋势
            </div>
            <TrendChart points={data.dailyPoints} period={period} />
          </div>
        )}

        {/* Two-column layout */}
        <div className="grid grid-cols-2 gap-14">
          {/* Left: Tool breakdown */}
          <div>
            <div className="text-[10px] font-semibold text-t-3 uppercase tracking-widest mb-5">
              工具占比
            </div>
            <ToolBreakdown stats={data.toolStats} />
          </div>

          {/* Right: Top tasks */}
          <div>
            <div className="text-[10px] font-semibold text-t-3 uppercase tracking-widest mb-5">
              活跃任务排行
            </div>
            <TopTasksTable tasks={data.topTasks} />
          </div>
        </div>
      </div>
    </>
  );
}

// ─── SVG Trend Chart ────────────────────────────────────────────

function TrendChart({
  points,
  period,
}: {
  points: DailyPoint[];
  period: InsightPeriod;
}) {
  const W = 600;
  const H = 140;
  const PAD_TOP = 10;
  const PAD_BOTTOM = 24;
  const PAD_LEFT = 0;
  const PAD_RIGHT = 0;

  const chartW = W - PAD_LEFT - PAD_RIGHT;
  const chartH = H - PAD_TOP - PAD_BOTTOM;

  const values = points.map((p) => p.totalDurationSecs / 60); // minutes
  const maxVal = Math.max(...values, 1);

  const coords = values.map((v, i) => {
    const x = PAD_LEFT + (points.length === 1 ? chartW / 2 : (i / (points.length - 1)) * chartW);
    const y = PAD_TOP + chartH - (v / maxVal) * chartH;
    return { x, y };
  });

  // Smooth bezier path
  const linePath = coords.length <= 1
    ? `M${coords[0]?.x ?? 0},${coords[0]?.y ?? H / 2}`
    : coords.reduce((acc, point, i) => {
        if (i === 0) return `M${point.x},${point.y}`;
        const prev = coords[i - 1];
        const cpx1 = prev.x + (point.x - prev.x) * 0.4;
        const cpx2 = point.x - (point.x - prev.x) * 0.4;
        return `${acc} C${cpx1},${prev.y} ${cpx2},${point.y} ${point.x},${point.y}`;
      }, "");

  const areaPath = `${linePath} L${coords[coords.length - 1]?.x ?? 0},${H} L${coords[0]?.x ?? 0},${H} Z`;

  return (
    <div className="w-full">
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full" style={{ height: 140 }}>
        {/* Grid lines */}
        {[0, 0.25, 0.5, 0.75, 1].map((frac) => {
          const y = PAD_TOP + chartH * (1 - frac);
          return (
            <line
              key={frac}
              x1={PAD_LEFT}
              y1={y}
              x2={W - PAD_RIGHT}
              y2={y}
              stroke="rgba(0,0,0,0.04)"
              strokeWidth={0.5}
            />
          );
        })}

        {/* Area fill */}
        <path d={areaPath} fill="rgba(0,122,255,0.05)" />

        {/* Line */}
        <path d={linePath} fill="none" stroke="#007aff" strokeWidth={1.5} strokeLinecap="round" />

        {/* Dots */}
        {coords.map((c, i) => (
          <circle key={i} cx={c.x} cy={c.y} r={2} fill="#007aff" />
        ))}

        {/* X labels */}
        {points.map((p, i) => {
          const x = coords[i].x;
          // Show fewer labels when there are many points
          const skip = points.length > 14 ? 4 : points.length > 7 ? 2 : 1;
          if (i % skip !== 0 && i !== points.length - 1) return null;
          return (
            <text
              key={i}
              x={x}
              y={H - 4}
              textAnchor="middle"
              className="fill-t-4"
              style={{ fontSize: 9, fontFamily: "SF Mono, Menlo, monospace" }}
            >
              {p.date}
            </text>
          );
        })}
      </svg>
    </div>
  );
}

// ─── Tool Breakdown (Screen Time style) ─────────────────────────

function ToolBreakdown({ stats }: { stats: ToolUsageStat[] }) {
  if (stats.length === 0) {
    return <div className="text-[12px] text-t-3">暂无工具使用数据</div>;
  }

  return (
    <div className="space-y-5">
      {stats.map((s) => (
        <div key={s.toolKey}>
          <div className="flex justify-between text-[12px] mb-1.5 font-medium">
            <span className="flex items-center gap-2">
              {getToolBrandIcon(s.toolKey, 14)}
              {s.label}
            </span>
            <span className="font-mono text-t-3">{s.durationPercent.toFixed(0)}%</span>
          </div>
          <div className="h-[3px] rounded-full bg-black/[0.04] overflow-hidden">
            <div
              className="h-full rounded-full transition-all duration-500"
              style={{
                width: `${Math.max(3, s.durationPercent)}%`,
                backgroundColor: s.color,
              }}
            />
          </div>
          <div className="flex gap-4 mt-1.5 text-[10px] text-t-3">
            <span>{s.taskCount} 个任务</span>
            <span>{fmtDuration(s.totalDurationSecs)}</span>
          </div>
        </div>
      ))}
    </div>
  );
}

// ─── Top Tasks Table ────────────────────────────────────────────

function TopTasksTable({ tasks }: { tasks: TopTask[] }) {
  if (tasks.length === 0) {
    return <div className="text-[12px] text-t-3">暂无任务记录</div>;
  }

  return (
    <table className="w-full text-[12px]">
      <thead>
        <tr className="text-left text-t-4 border-b border-black/[0.06] text-[9px] uppercase tracking-wider">
          <th className="pb-2 font-semibold">任务描述</th>
          <th className="pb-2 font-semibold text-right w-[54px]">时长</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-black/[0.03]">
        {tasks.map((t, i) => {
          const meta = getToolMeta(t.tool);
          return (
            <tr key={t.id} className={i >= 3 ? "opacity-40" : ""}>
              <td className="py-2.5 pr-3">
                <div className="flex items-center gap-1.5">
                  {t.tool && getToolBrandIcon(t.tool.toLowerCase().replace(/_/g, "-"), 13)}
                  <span className="font-medium truncate max-w-[180px]">{t.prompt}</span>
                </div>
              </td>
              <td className="py-2.5 text-right font-mono text-t-3">{fmtDuration(t.durationSecs)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
