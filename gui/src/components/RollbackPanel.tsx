/**
 * RollbackPanel — "读档"确认弹窗（居中 sheet modal）
 */
import { useState, useEffect, useRef } from "react";
import { RotateCcw, CheckCircle, AlertTriangle, Info, X } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { previewRollback, rollbackTask, type UndoPreviewInfo, type RestoreProgressInfo } from "../lib/tauri";
import { fileName } from "../lib/format";

interface Props {
  taskId: string;
  isMonitoringWindow: boolean;
  windowLabel?: string;
  onClose: () => void;
  onRolledBack: () => void;
}

export default function RollbackPanel({
  taskId,
  isMonitoringWindow,
  windowLabel,
  onClose,
  onRolledBack,
}: Props) {
  const [preview, setPreview] = useState<UndoPreviewInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [rolling, setRolling] = useState(false);
  const [restoreProgress, setRestoreProgress] = useState<RestoreProgressInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);
  const unlistenRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    previewRollback(taskId)
      .then(setPreview)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [taskId]);

  // Subscribe to restore-progress events while the panel is mounted
  useEffect(() => {
    let active = true;
    listen<RestoreProgressInfo>("restore-progress", (event) => {
      if (!active) return;
      if (event.payload.task_id === taskId) {
        setRestoreProgress(event.payload);
      }
    }).then((unlisten) => {
      unlistenRef.current = unlisten;
    });
    return () => {
      active = false;
      unlistenRef.current?.();
    };
  }, [taskId]);

  const handleRollback = async () => {
    setRolling(true);
    setRestoreProgress({
      is_running: true,
      phase: "restoring-files",
      task_id: taskId,
      dir_path: null,
      total_files: preview?.total_changes ?? 0,
      processed_files: 0,
      restored_files: 0,
      deleted_files: 0,
      failed_files: 0,
      current_path: null,
    });
    setError(null);
    try {
      const res = await rollbackTask(taskId);
      const anyDone = res.files_restored > 0 || res.files_deleted > 0;
      const hasFailures = res.failures.length > 0;

      if (!hasFailures && !anyDone) {
        setError("此任务没有可恢复的文件变更");
      } else if (!hasFailures) {
        setSuccess(true);
        setTimeout(onRolledBack, 1200);
      } else if (anyDone) {
        const msg = res.failures.map(([p, e]) => `${fileName(p)}: ${e}`).join("\n");
        setError(`部分文件读档失败：\n${msg}`);
        setTimeout(onRolledBack, 2000);
      } else {
        const msg = res.failures.map(([p, e]) => `${fileName(p)}: ${e}`).join("\n");
        setError(msg || "读档失败，请重试");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setRolling(false);
    }
  };

  const actionDesc = isMonitoringWindow
    ? `将 ${windowLabel ?? "此时间段"} 内所有文件与目录回到该存档点之前的版本`
    : "将此次 AI 任务涉及的所有文件回到操作执行之前的版本";

  const handleOverlayClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.target === e.currentTarget && !rolling) onClose();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" && !rolling) onClose();
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ background: "rgba(0,0,0,0.18)" }}
      onClick={handleOverlayClick}
      onKeyDown={handleKeyDown}
      tabIndex={-1}
      ref={(el) => el?.focus()}
    >
      <div className="bg-white rounded-xl shadow-[0_20px_60px_rgba(0,0,0,0.2),0_0_0_0.5px_rgba(0,0,0,0.1)] w-[420px] max-h-[520px] flex flex-col overflow-hidden">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-border flex-shrink-0">
          <div className="flex items-center gap-2">
            <RotateCcw className="w-4 h-4 text-sys-blue" />
            <span className="text-[14px] font-semibold text-t-1">确认读档</span>
          </div>
          <button
            onClick={onClose}
            disabled={rolling}
            className="w-[22px] h-[22px] rounded-full bg-bg-active text-t-2 flex items-center justify-center text-[11px] hover:bg-border cursor-default disabled:opacity-40"
          >
            <X className="w-3 h-3" />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4">
          <p className="text-[12px] text-t-2 mb-3">{actionDesc}</p>

          {/* Notes */}
          <div className="mb-3 space-y-2">
            <div className="flex gap-2 text-[11px] text-t-3 items-start">
              <CheckCircle className="w-3.5 h-3.5 text-sys-green flex-shrink-0 mt-0.5" />
              <span>可随时再次读档，不限次数，操作可撤销</span>
            </div>
            <div className="flex gap-2 text-[11px] text-t-3 items-start">
              <Info className="w-3.5 h-3.5 text-t-4 flex-shrink-0 mt-0.5" />
              <span>读档不会改动不在此存档点内的文件</span>
            </div>
            {preview && preview.files_to_delete.length > 0 && (
              <div className="flex gap-2 text-[11px] text-sys-red items-start">
                <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
                <span>此存档点内有新建的文件，读档后这些文件将被<b>删除</b></span>
              </div>
            )}
          </div>

          {/* Restore progress — shown while rolling is in progress.
               NOTE: do NOT gate on restoreProgress.is_running here.  The backend emits
               {is_running:false, phase:"done"} before returning the command response, so
               conditioning on is_running would cause the bar to disappear while rolling is
               still true, creating a blank gap before the success message appears. */}
          {rolling && restoreProgress && (
            <div className="mb-3 space-y-2">
              <div className="flex items-center justify-between text-[11px] text-t-3">
                <span className="flex items-center gap-1.5">
                  <span className={`inline-block w-1.5 h-1.5 rounded-full bg-sys-blue ${restoreProgress.is_running ? "animate-pulse" : ""}`} />
                  {restoreProgress.phase === "syncing-database"
                    ? "正在同步数据库…"
                    : restoreProgress.phase === "finalizing"
                      ? "正在收尾…"
                      : restoreProgress.phase === "done"
                        ? "恢复完成"
                        : "正在恢复文件…"}
                </span>
                <span className="tabular-nums">
                  {restoreProgress.processed_files.toLocaleString()} /{" "}
                  {restoreProgress.total_files.toLocaleString()}
                </span>
              </div>
              <div className="h-1 bg-bg-active rounded-full overflow-hidden">
                <div
                  className="h-full rounded-full bg-sys-blue transition-all duration-200"
                  style={{
                    width:
                      restoreProgress.total_files > 0
                        ? `${Math.min(100, Math.round((restoreProgress.processed_files / restoreProgress.total_files) * 100))}%`
                        : "0%",
                  }}
                />
              </div>
              {restoreProgress.current_path && (
                <div className="text-[10px] text-t-4 font-mono truncate">
                  {fileName(restoreProgress.current_path)}
                </div>
              )}
            </div>
          )}

          {/* Preview / Status */}
          {loading ? (
            <div className="py-2 text-[12px] text-t-3 flex items-center gap-2">
              <span className="animate-spin">◐</span> 分析影响范围...
            </div>
          ) : error ? (
            <div className="py-2 text-[12px] text-sys-red whitespace-pre-wrap bg-sys-red/5 rounded-md px-4 py-3">
              {error}
            </div>
          ) : success ? (
            <div className="py-2 flex items-center gap-2 text-[13px] text-sys-green font-medium">
              <CheckCircle className="w-4 h-4" />
              读档完成，文件已还原
            </div>
          ) : preview ? (
            <div className="space-y-2">
              {preview.files_to_restore.length > 0 && (
                <div>
                  <div className="text-[11px] text-t-3 mb-1">
                    将还原 {preview.files_to_restore.length} 个文件:
                  </div>
                  <ul className="space-y-0.5">
                    {preview.files_to_restore.slice(0, 5).map((p) => (
                      <li key={p} className="text-[11px] font-mono text-t-2 flex items-center gap-1.5">
                        <span className="w-[6px] h-[6px] rounded-full bg-sys-amber flex-shrink-0" />
                        {fileName(p)}
                      </li>
                    ))}
                    {preview.files_to_restore.length > 5 && (
                      <li className="text-[10px] text-t-4">…另 {preview.files_to_restore.length - 5} 个</li>
                    )}
                  </ul>
                </div>
              )}
              {preview.files_to_delete.length > 0 && (
                <div>
                  <div className="text-[11px] text-t-3 mb-1">
                    将删除 {preview.files_to_delete.length} 个新建的文件:
                  </div>
                  <ul className="space-y-0.5">
                    {preview.files_to_delete.slice(0, 5).map((p) => (
                      <li key={p} className="text-[11px] font-mono text-t-2 flex items-center gap-1.5">
                        <span className="w-[6px] h-[6px] rounded-full bg-sys-red flex-shrink-0" />
                        {fileName(p)}
                      </li>
                    ))}
                    {preview.files_to_delete.length > 5 && (
                      <li className="text-[10px] text-t-4">…另 {preview.files_to_delete.length - 5} 个</li>
                    )}
                  </ul>
                </div>
              )}
              {preview.files_to_restore.length === 0 && preview.files_to_delete.length === 0 && (
                <p className="text-[12px] text-t-3">没有需要还原的文件</p>
              )}
            </div>
          ) : null}
        </div>

        {/* Footer — right-aligned macOS-style dual buttons */}
        {!success && !loading && (
          <div className="flex items-center justify-end gap-3 px-5 py-3 border-t border-border flex-shrink-0">
            <button
              onClick={onClose}
              disabled={rolling}
              className="text-[12px] text-t-2 font-medium hover:text-t-1 transition-colors disabled:opacity-40"
            >
              取消
            </button>
            <button
              onClick={handleRollback}
              disabled={rolling || !!error || (preview?.files_to_restore.length === 0 && preview?.files_to_delete.length === 0)}
              className="px-4 py-1.5 rounded-md bg-sys-red text-white text-[12px] font-medium hover:opacity-90 transition-opacity disabled:opacity-40"
            >
              {rolling
                ? restoreProgress && restoreProgress.total_files > 0
                  ? `读档中 ${Math.min(100, Math.round((restoreProgress.processed_files / restoreProgress.total_files) * 100))}%`
                  : "读档中..."
                : "确认读档"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
