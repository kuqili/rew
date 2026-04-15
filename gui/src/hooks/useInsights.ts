import { useState, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getInsights,
  type InsightsDataInfo,
} from "../lib/tauri";
import { getToolMeta } from "../lib/tools";

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

// ─── Hook ───────────────────────────────────────────────────────

function mapInsightsData(raw: InsightsDataInfo): InsightsData {
  return {
    period: raw.period as InsightPeriod,
    totalTokens: raw.total_tokens,
    totalDurationSecs: raw.total_duration_secs,
    totalCostUsd: raw.total_cost_usd,
    totalTasks: raw.total_tasks,
    totalFilesChanged: raw.total_files_changed,
    toolStats: raw.tool_stats.map((stat) => {
      const meta = getToolMeta(stat.tool_key);
      return {
        toolKey: stat.tool_key,
        label: meta?.label ?? stat.tool_key,
        color: meta?.color ?? "#007aff",
        taskCount: stat.task_count,
        totalDurationSecs: stat.total_duration_secs,
        totalTokens: stat.total_tokens,
        totalCostUsd: stat.total_cost_usd,
        durationPercent: stat.duration_percent,
      };
    }),
    dailyPoints: raw.daily_points.map((point) => ({
      date: point.date,
      dateISO: point.date_iso,
      taskCount: point.task_count,
      totalDurationSecs: point.total_duration_secs,
      totalTokens: point.total_tokens,
    })),
    topTasks: raw.top_tasks.map((task) => ({
      id: task.id,
      prompt: task.prompt,
      tool: task.tool,
      durationSecs: task.duration_secs,
      changesCount: task.changes_count,
    })),
  };
}

export function useInsights(period: InsightPeriod): {
  data: InsightsData | null;
  loading: boolean;
  refresh: () => void;
} {
  const [data, setData] = useState<InsightsData | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const next = await getInsights(period);
      setData(mapInsightsData(next));
    } catch {
      setData(null);
    } finally {
      setLoading(false);
    }
  }, [period]);

  useEffect(() => {
    setLoading(true);
    refresh();
    const timer = setInterval(refresh, 30000);
    const unlistenTask = listen("task-updated", () => refresh());
    return () => {
      clearInterval(timer);
      unlistenTask.then((fn) => fn());
    };
  }, [refresh]);

  return { data, loading, refresh };
}
