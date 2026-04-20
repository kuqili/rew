import { useState, useCallback, useMemo, useEffect } from "react";
import Sidebar from "./Sidebar";
import TaskTimeline from "./TaskTimeline";
import TaskDetail from "./TaskDetail";
import SettingsPanel from "./SettingsPanel";
import InsightsPanel from "./InsightsPanel";
import BatchProgressBanner from "./BatchProgressBanner";
import { useTasks } from "../hooks/useTasks";
import { useBatchProgress } from "../hooks/useBatchProgress";
import { getToolMeta } from "../lib/tools";
import { check } from "@tauri-apps/plugin-updater";

export type ViewMode = "all" | "ai" | "insights";

export default function MainLayout() {
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [selectedDir, setSelectedDir] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const [viewMode, setViewMode] = useState<ViewMode>("all");
  const [toolFilter, setToolFilter] = useState<string | null>(null);
  const [hasUpdate, setHasUpdate] = useState(false);

  // 启动时静默检查更新
  useEffect(() => {
    check().then((update) => {
      if (update) setHasUpdate(true);
    }).catch(() => {});
  }, []);

  // Load tasks to compute active tools for sidebar
  const { tasks: allTasks } = useTasks(selectedDir, { enabled: viewMode !== "insights" });

  // Batch processing progress for large FSEvent batches
  const batchProgress = useBatchProgress();

  const activeTools = useMemo(() => {
    const seen = new Map<string, string>();
    for (const t of allTasks) {
      if (!t.tool || t.tool === "文件监听") continue;
      const key = t.tool.toLowerCase().replace(/_/g, "-");
      if (!seen.has(key)) {
        const meta = getToolMeta(t.tool);
        seen.set(key, meta?.label ?? t.tool);
      }
    }
    return Array.from(seen.entries()).map(([key, label]) => ({ key, label }));
  }, [allTasks]);

  const handleViewModeChange = useCallback((mode: ViewMode) => {
    setViewMode(mode);
    setToolFilter(null);
    setSelectedTaskId(null);
  }, []);

  const handleSelectDir = useCallback((dir: string | null) => {
    setSelectedDir(dir);
    setSelectedTaskId(null);
  }, []);

  return (
    <div className="flex flex-col h-screen bg-white overflow-hidden">
      {/* Batch processing progress banner (large file deletions/creates) */}
      <BatchProgressBanner progress={batchProgress} />

      {/* Settings modal overlay */}
      {showSettings && (
        <div className="modal-overlay">
          <div className="w-[720px] h-[540px] bg-white rounded-[10px] shadow-[0_20px_60px_rgba(0,0,0,0.2),0_0_0_0.5px_rgba(0,0,0,0.1)] flex overflow-hidden">
            <SettingsPanel onClose={() => setShowSettings(false)} />
          </div>
        </div>
      )}

      {/* Drag region: 52px (v7) - 28px (native titlebar) = 24px */}
      <div data-tauri-drag-region className="h-[24px] flex-shrink-0 w-full" />

      {/* Three columns below titlebar */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left: Sidebar (220px) */}
        <Sidebar
          selectedDir={selectedDir}
          onSelectDir={handleSelectDir}
          viewMode={viewMode}
          onViewModeChange={handleViewModeChange}
          onOpenSettings={() => setShowSettings(true)}
          toolFilter={toolFilter}
          onToolFilterChange={setToolFilter}
          activeTools={activeTools}
          hasUpdate={hasUpdate}
        />

        {viewMode === "insights" ? (
          /* Insights full panel (replaces timeline + detail) */
          <div className="flex-1 flex flex-col min-w-0 overflow-hidden bg-white">
            <InsightsPanel />
          </div>
        ) : (
          <>
            {/* Middle: Timeline (400px) */}
            <div className="w-[400px] flex-shrink-0 border-r border-border bg-white flex flex-col overflow-hidden">
              <TaskTimeline
                selectedId={selectedTaskId}
                onSelect={setSelectedTaskId}
                dirFilter={selectedDir}
                viewMode={viewMode}
                toolFilter={toolFilter}
                key={`${refreshKey}-${selectedDir ?? "all"}-${viewMode}`}
              />
            </div>

            {/* Right: Inspector (flex-1) */}
            <div className="flex-1 flex flex-col min-w-0 overflow-hidden bg-white">
              {selectedTaskId ? (
                <TaskDetail
                  key={`${selectedTaskId}-${selectedDir ?? "all"}`}
                  taskId={selectedTaskId}
                  dirFilter={selectedDir}
                  onTaskUpdated={() => setRefreshKey((k) => k + 1)}
                  onBack={() => setSelectedTaskId(null)}
                />
              ) : (
                <EmptyInspector />
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function EmptyInspector() {
  return (
    <div className="h-full flex flex-col items-center justify-center select-none text-center px-8 bg-bg-grouped">
      <div className="w-16 h-16 rounded-2xl bg-white flex items-center justify-center mb-4">
        <svg className="w-8 h-8 text-t-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 00-3.375-3.375h-1.5A1.125 1.125 0 0113.5 7.125v-1.5a3.375 3.375 0 00-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 00-9-9z" />
        </svg>
      </div>
      <div className="text-[14px] text-t-2 font-medium mb-1.5">选择一条存档记录</div>
      <div className="text-[12px] text-t-3 max-w-[240px] leading-relaxed">
        点击左侧时间线中的任意记录，查看该存档点内的文件变更，或从中读档回到历史状态。
      </div>
    </div>
  );
}
