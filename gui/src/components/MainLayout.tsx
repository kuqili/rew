import { useState, useRef, useCallback } from "react";
import Sidebar from "./Sidebar";
import Toolbar from "./Toolbar";
import TaskTimeline from "./TaskTimeline";
import TaskDetail from "./TaskDetail";
import SettingsPanel from "./SettingsPanel";

export default function MainLayout() {
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [selectedDir, setSelectedDir] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);

  // Sidebar width (px), user can drag to resize
  const [sidebarWidth, setSidebarWidth] = useState(200);

  // Vertical split: height of the top timeline pane in px
  const [topHeight, setTopHeight] = useState(260);
  const splitAreaRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  const onDividerMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    const onMove = (me: MouseEvent) => {
      if (!dragging.current || !splitAreaRef.current) return;
      const rect = splitAreaRef.current.getBoundingClientRect();
      const newH = Math.max(120, Math.min(me.clientY - rect.top, rect.height - 180));
      setTopHeight(newH);
    };
    const onUp = () => {
      dragging.current = false;
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }, []);

  return (
    <div className="flex h-screen bg-white overflow-hidden">
      {/* Left icon sidebar */}
      <Sidebar
        selectedDir={selectedDir}
        onSelectDir={setSelectedDir}
        onOpenSettings={() => setShowSettings(true)}
        width={sidebarWidth}
        onWidthChange={setSidebarWidth}
      />

      {/* Main content area */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Toolbar */}
        <Toolbar
          onBack={undefined}
          onSettings={() => setShowSettings(true)}
        />

        {/* Settings overlay */}
        {showSettings && (
          <div className="absolute inset-0 z-50 bg-white">
            <SettingsPanel onClose={() => setShowSettings(false)} />
          </div>
        )}

        {/* Three-pane body: top timeline | drag | bottom detail */}
        <div ref={splitAreaRef} className="flex-1 flex flex-col overflow-hidden min-h-0">

          {/* ── Top pane: Timeline ───────────────────────────────── */}
          <div
            className="flex-shrink-0 overflow-hidden border-b border-surface-border"
            style={{ height: topHeight }}
          >
            <TaskTimeline
              selectedId={selectedTaskId}
              onSelect={setSelectedTaskId}
              dirFilter={selectedDir}
              key={`${refreshKey}-${selectedDir ?? "all"}`}
            />
          </div>

          {/* ── Drag handle ─────────────────────────────────────── */}
          <div
            onMouseDown={onDividerMouseDown}
            className="flex-shrink-0 h-[5px] bg-surface-border/30 hover:bg-st-blue/30 cursor-row-resize flex items-center justify-center group transition-colors"
          >
            <div className="w-10 h-[3px] rounded-full bg-surface-border/60 group-hover:bg-st-blue/50 transition-colors" />
          </div>

          {/* ── Bottom pane: File list + Diff ───────────────────── */}
          <div className="flex-1 overflow-hidden min-h-0">
            {selectedTaskId ? (
              <TaskDetail
                key={`${selectedTaskId}-${selectedDir ?? "all"}`}
                taskId={selectedTaskId}
                dirFilter={selectedDir}
                onTaskUpdated={() => setRefreshKey((k) => k + 1)}
                onBack={() => setSelectedTaskId(null)}
              />
            ) : (
              <EmptyDetail />
            )}
          </div>

        </div>
      </div>
    </div>
  );
}

function EmptyDetail() {
  return (
    <div className="h-full flex flex-col items-center justify-center select-none">
      <div className="text-[32px] opacity-10 mb-3">📂</div>
      <div className="text-[13px] text-ink-muted font-medium mb-1">选择一条存档记录</div>
      <div className="text-[11px] text-ink-faint max-w-[260px] text-center leading-relaxed">
        点击上方时间线中的任意记录，查看该存档点内的文件变更，或从中读档回到历史状态。
      </div>
    </div>
  );
}
