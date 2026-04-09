import { useState } from "react";
import { useStatus, useTasks } from "../hooks/useTasks";
import { useScanProgress } from "../hooks/useScanProgress";
import { addWatchDir, removeWatchDir, type DirScanStatus } from "../lib/tauri";

export default function Sidebar() {
  const status = useStatus();
  const { tasks } = useTasks();
  const scanProgress = useScanProgress();

  const handleAddDir = async () => {
    try {
      // Use Tauri dialog to pick a directory
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ directory: true, multiple: false });
      if (selected) {
        await addWatchDir(selected as string);
      }
    } catch (e) {
      console.error("Failed to add directory:", e);
    }
  };

  const handleRemoveDir = async (path: string) => {
    const name = path.split("/").pop() || path;
    if (confirm(`停止保护「${name}」？\n已备份的文件不会被删除。`)) {
      try {
        await removeWatchDir(path);
      } catch (e) {
        console.error("Failed to remove directory:", e);
      }
    }
  };

  return (
    <aside className="w-[180px] min-w-[180px] bg-surface-sidebar flex flex-col h-full shadow-sidebar select-none">
      {/* Drag region / title */}
      <div data-tauri-drag-region className="h-[38px] flex items-end px-4 pb-1">
        <span className="text-2xs font-semibold text-ink-muted uppercase tracking-wider">
          WORKSPACE
        </span>
      </div>

      {/* Navigation */}
      <nav className="mt-1 flex-1">
        <div className="nav-item active">
          <span className="nav-icon">🕐</span>
          <span>历史</span>
        </div>
        <div className="nav-item">
          <span className="nav-icon">🔍</span>
          <span>搜索</span>
        </div>

        <div className="mt-6 px-4 mb-2">
          <span className="text-2xs font-semibold text-ink-muted uppercase tracking-wider">
            保护状态
          </span>
        </div>

        <div className="nav-item">
          <span className="nav-icon">🛡️</span>
          <span>
            {status?.running ? "保护中" : "已暂停"}
          </span>
          {status?.running && (
            <span className="ml-auto w-2 h-2 rounded-full bg-status-green" />
          )}
        </div>

        {/* Protected directories with scan status */}
        <div className="mt-5">
          <div className="px-4 mb-2 flex items-center justify-between">
            <span className="text-2xs font-semibold text-ink-muted uppercase tracking-wider">
              保护目录
            </span>
            <button
              onClick={handleAddDir}
              className="w-4 h-4 flex items-center justify-center text-xs text-ink-faint hover:text-ink-primary hover:bg-surface-hover rounded transition-colors"
              title="添加保护目录"
            >
              +
            </button>
          </div>

          {/* Scan progress banner */}
          {scanProgress?.is_scanning && (
            <div className="mx-3 mb-2 px-2 py-1.5 bg-surface-hover rounded text-2xs text-ink-secondary">
              <div className="flex items-center gap-1.5 mb-1">
                <span className="inline-block animate-spin text-[10px]">◐</span>
                <span>正在初始化保护...</span>
              </div>
              <OverallProgressBar dirs={scanProgress.dirs} />
            </div>
          )}

          <div className="space-y-0.5">
            {(scanProgress?.dirs || []).map((dir) => (
              <DirEntry key={dir.path} dir={dir} onRemove={handleRemoveDir} />
            ))}
            {/* Fallback: show from status if scan progress not loaded yet */}
            {!scanProgress && status?.watch_dirs.map((dir, i) => {
              const name = dir.split("/").pop() || dir;
              return (
                <div
                  key={i}
                  className="flex items-center gap-2 px-4 py-1 text-2xs text-ink-secondary mx-2"
                  title={dir}
                >
                  <span className="opacity-40">○</span>
                  <span className="truncate">{name}</span>
                </div>
              );
            })}
          </div>
        </div>
      </nav>

      {/* Bottom */}
      <div className="p-3 border-t border-surface-border">
        <div className="text-2xs text-ink-muted">
          {tasks.length > 0
            ? `${tasks.length} 次 AI 操作已记录`
            : "等待 AI 操作..."}
        </div>
      </div>
    </aside>
  );
}

function DirEntry({
  dir,
  onRemove,
}: {
  dir: DirScanStatus;
  onRemove: (path: string) => void;
}) {
  const [hover, setHover] = useState(false);

  return (
    <div
      className="group flex items-center gap-2 px-4 py-1 text-2xs text-ink-secondary hover:bg-surface-hover rounded-sm mx-2 cursor-default"
      title={`${dir.path}${
        dir.status === "complete"
          ? `\n✓ 已保护 (${dir.files_done.toLocaleString()} 文件)`
          : dir.status === "scanning"
            ? `\n扫描中 ${dir.percent.toFixed(0)}% (${dir.files_done.toLocaleString()}/${dir.files_total.toLocaleString()})`
            : "\n等待扫描"
      }`}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      {/* Status icon */}
      <span className="w-3 flex-shrink-0 text-center text-[10px]">
        {dir.status === "complete" && (
          <span className="text-status-green">✓</span>
        )}
        {dir.status === "scanning" && (
          <span className="inline-block animate-spin">◐</span>
        )}
        {dir.status === "pending" && (
          <span className="opacity-30">○</span>
        )}
      </span>

      <span className="truncate flex-1">{dir.name}</span>

      {/* Scanning: show percent */}
      {dir.status === "scanning" && (
        <span className="text-[10px] text-ink-faint tabular-nums flex-shrink-0">
          {dir.percent.toFixed(0)}%
        </span>
      )}

      {/* Hover: show remove button */}
      {hover && dir.status !== "scanning" && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onRemove(dir.path);
          }}
          className="text-[10px] text-ink-faint hover:text-status-red transition-colors flex-shrink-0"
        >
          ×
        </button>
      )}
    </div>
  );
}

function OverallProgressBar({ dirs }: { dirs: DirScanStatus[] }) {
  const totalFiles = dirs.reduce((s, d) => s + d.files_total, 0);
  const doneFiles = dirs.reduce((s, d) => s + d.files_done, 0);
  const percent = totalFiles > 0 ? (doneFiles / totalFiles) * 100 : 0;

  return (
    <div className="w-full bg-surface-border rounded-full h-1 overflow-hidden">
      <div
        className="h-full bg-st-blue rounded-full transition-all duration-300"
        style={{ width: `${Math.min(percent, 100)}%` }}
      />
    </div>
  );
}
