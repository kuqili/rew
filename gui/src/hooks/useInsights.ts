import { useState, useEffect, useMemo, useCallback } from "react";
import { listTasks, type TaskInfo, type TaskStatsInfo } from "../lib/tauri";
import { getToolMeta, TOOL_REGISTRY } from "../lib/tools";

// ─── Types ──────────────────────────────────────────────────────

export type InsightPeriod = "day" | "week" | "month";

export interface ToolUsageStat {
  toolKey: string;
  label: string;
  color: string;
  taskCount: number;
  totalDurationSecs: number;
  totalTokens: number;
  totalCostUsd: number;
  /** Percentage of total duration */
  durationPercent: number;
}

export interface DailyPoint {
  date: string;        // "04/08" format
  dateISO: string;     // "2026-04-08"
  taskCount: number;
  totalDurationSecs: number;
  totalTokens: number;
}

export interface TopTask {
  id: string;
  prompt: string;
  tool: string | null;
  durationSecs: number;
  changesCount: number;
}

export interface InsightsData {
  period: InsightPeriod;
  /** Aggregate metrics */
  totalTokens: number;
  totalDurationSecs: number;
  totalCostUsd: number;
  totalTasks: number;
  totalFilesChanged: number;
  /** Per-tool breakdown */
  toolStats: ToolUsageStat[];
  /** Daily trend data */
  dailyPoints: DailyPoint[];
  /** Top tasks by duration */
  topTasks: TopTask[];
}

// ─── Helpers ────────────────────────────────────────────────────

function getPeriodRange(period: InsightPeriod): { start: Date; end: Date } {
  const now = new Date();
  const end = new Date(now);
  end.setHours(23, 59, 59, 999);

  const start = new Date(now);
  start.setHours(0, 0, 0, 0);

  switch (period) {
    case "day":
      // Today only
      break;
    case "week":
      start.setDate(start.getDate() - 6);
      break;
    case "month":
      start.setDate(start.getDate() - 29);
      break;
  }
  return { start, end };
}

function toDateKey(d: Date): string {
  return d.toLocaleDateString("sv"); // "2026-04-08"
}

function toShortDate(iso: string): string {
  const [, m, d] = iso.split("-");
  return `${m}/${d}`;
}

function taskDurationSecs(task: TaskInfo): number {
  if (!task.completed_at) return 0;
  const start = new Date(task.started_at).getTime();
  const end = new Date(task.completed_at).getTime();
  const secs = (end - start) / 1000;
  return Math.max(0, secs);
}

function isAiTask(task: TaskInfo): boolean {
  return !!task.tool && task.tool !== "文件监听" && task.tool !== "手动存档";
}

// ─── Hook ───────────────────────────────────────────────────────

export function useInsights(period: InsightPeriod, dirFilter?: string | null): {
  data: InsightsData | null;
  loading: boolean;
  refresh: () => void;
} {
  const [tasks, setTasks] = useState<TaskInfo[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const all = await listTasks(dirFilter ?? undefined);
      setTasks(all || []);
    } catch {
      setTasks([]);
    } finally {
      setLoading(false);
    }
  }, [dirFilter]);

  useEffect(() => {
    setLoading(true);
    refresh();
    const timer = setInterval(refresh, 10000);
    return () => clearInterval(timer);
  }, [refresh]);

  const data = useMemo<InsightsData | null>(() => {
    if (loading && tasks.length === 0) return null;

    const { start, end } = getPeriodRange(period);

    // Filter tasks within period + AI tasks only
    const aiTasks = tasks.filter((t) => {
      if (!isAiTask(t)) return false;
      const ts = new Date(t.started_at);
      return ts >= start && ts <= end;
    });

    // Aggregate per-tool stats
    const toolMap = new Map<string, {
      label: string;
      color: string;
      taskCount: number;
      totalDurationSecs: number;
      totalTokens: number;
      totalCostUsd: number;
    }>();

    let totalDuration = 0;
    let totalTokens = 0;
    let totalCost = 0;
    let totalFiles = 0;

    for (const t of aiTasks) {
      const dur = taskDurationSecs(t);
      totalDuration += dur;
      totalFiles += t.changes_count ?? 0;

      // Simulated token/cost (from task_stats if available, otherwise estimate)
      // In production these come from task_stats table
      const estimatedTokens = Math.round((t.total_lines_added + t.total_lines_removed) * 25 + 500);
      const estimatedCost = estimatedTokens * 0.000003;

      totalTokens += estimatedTokens;
      totalCost += estimatedCost;

      const key = (t.tool || "unknown").toLowerCase().replace(/_/g, "-");
      const meta = getToolMeta(t.tool);
      const existing = toolMap.get(key);
      if (existing) {
        existing.taskCount++;
        existing.totalDurationSecs += dur;
        existing.totalTokens += estimatedTokens;
        existing.totalCostUsd += estimatedCost;
      } else {
        toolMap.set(key, {
          label: meta?.label ?? t.tool ?? "Unknown",
          color: meta?.color ?? "#007aff",
          taskCount: 1,
          totalDurationSecs: dur,
          totalTokens: estimatedTokens,
          totalCostUsd: estimatedCost,
        });
      }
    }

    const toolStats: ToolUsageStat[] = Array.from(toolMap.entries())
      .map(([toolKey, stat]) => ({
        toolKey,
        ...stat,
        durationPercent: totalDuration > 0 ? (stat.totalDurationSecs / totalDuration) * 100 : 0,
      }))
      .sort((a, b) => b.totalDurationSecs - a.totalDurationSecs);

    // Daily trend points
    const dayMap = new Map<string, DailyPoint>();
    const cursor = new Date(start);
    while (cursor <= end) {
      const key = toDateKey(cursor);
      dayMap.set(key, {
        date: toShortDate(key),
        dateISO: key,
        taskCount: 0,
        totalDurationSecs: 0,
        totalTokens: 0,
      });
      cursor.setDate(cursor.getDate() + 1);
    }

    for (const t of aiTasks) {
      const key = toDateKey(new Date(t.started_at));
      const point = dayMap.get(key);
      if (point) {
        point.taskCount++;
        point.totalDurationSecs += taskDurationSecs(t);
        point.totalTokens += Math.round((t.total_lines_added + t.total_lines_removed) * 25 + 500);
      }
    }

    const dailyPoints = Array.from(dayMap.values());

    // Top tasks by duration
    const topTasks: TopTask[] = aiTasks
      .map((t) => ({
        id: t.id,
        prompt: t.prompt || t.summary || "未命名任务",
        tool: t.tool,
        durationSecs: taskDurationSecs(t),
        changesCount: t.changes_count ?? 0,
      }))
      .sort((a, b) => b.durationSecs - a.durationSecs)
      .slice(0, 5);

    return {
      period,
      totalTokens,
      totalDurationSecs: totalDuration,
      totalCostUsd: totalCost,
      totalTasks: aiTasks.length,
      totalFilesChanged: totalFiles,
      toolStats,
      dailyPoints,
      topTasks,
    };
  }, [tasks, period, loading]);

  return { data, loading, refresh };
}
