import { useEffect, useRef, useState } from "react";
import type { BatchProgressState } from "../hooks/useBatchProgress";

interface BatchProgressBannerProps {
  progress: BatchProgressState;
}

export default function BatchProgressBanner({ progress }: BatchProgressBannerProps) {
  const [visible, setVisible] = useState(false);
  // useRef avoids stale-closure: the timer handle is always the latest value
  // without needing to be listed in useEffect's dependency array.
  const dismissTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (progress.active) {
      setVisible(true);
      // Cancel any pending auto-dismiss so an in-progress batch is never hidden
      if (dismissTimerRef.current) {
        clearTimeout(dismissTimerRef.current);
        dismissTimerRef.current = null;
      }
    } else {
      // Batch just completed (total may be 0 if all changes were deduped) —
      // auto-dismiss after 3 seconds regardless.
      dismissTimerRef.current = setTimeout(() => {
        setVisible(false);
      }, 3000);
    }
    return () => {
      if (dismissTimerRef.current) {
        clearTimeout(dismissTimerRef.current);
      }
    };
  }, [progress.active, progress.total]);

  if (!visible) return null;

  const pct =
    progress.total > 0 ? Math.min(100, Math.round((progress.processed / progress.total) * 100)) : 0;

  return (
    <div
      className="fixed bottom-4 left-1/2 -translate-x-1/2 z-50
                 bg-[#1a1a1a] text-white rounded-xl shadow-2xl
                 px-5 py-3 flex flex-col gap-2 w-[420px] max-w-[90vw]
                 animate-in fade-in slide-in-from-bottom-2 duration-300"
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2 min-w-0">
          {progress.active ? (
            <span className="w-2 h-2 rounded-full bg-blue-400 flex-shrink-0 animate-pulse" />
          ) : (
            <span className="w-2 h-2 rounded-full bg-green-400 flex-shrink-0" />
          )}
          <span className="text-[13px] font-medium truncate">
            {!progress.active
              ? "文件变更记录完成"
              : progress.phase === "planning"
              ? "正在分析文件变更"
              : "正在写入数据库"}
          </span>
        </div>
        <span className="text-[12px] text-gray-400 flex-shrink-0 tabular-nums">
          {progress.total.toLocaleString()} 个文件
        </span>
      </div>

      {/* Progress bar */}
      <div className="h-1 bg-white/10 rounded-full overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-300"
          style={{
            width: `${pct}%`,
            backgroundColor: !progress.active
              ? "#4ade80"
              : progress.phase === "planning"
              ? "#a78bfa"
              : "#60a5fa",
          }}
        />
      </div>

      <div className="flex items-center justify-between text-[11px] text-gray-400 tabular-nums">
        <span>
          {progress.processed.toLocaleString()} / {progress.total.toLocaleString()}
        </span>
        <span className="flex items-center gap-1.5">
          {progress.active && (
            <span className="text-gray-500">
              {progress.phase === "planning" ? "规划中" : "写入中"}
            </span>
          )}
          {pct}%
        </span>
      </div>
    </div>
  );
}
