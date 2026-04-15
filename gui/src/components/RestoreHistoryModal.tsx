/**
 * RestoreHistoryModal — 恢复历史弹窗（sheet modal）
 * 展示某个任务的所有恢复操作记录，按时间倒序。
 */
import { X, CheckCircle, AlertTriangle, Clock } from "lucide-react";
import type { RestoreOperationInfo } from "../lib/tauri";

function restoreStatusLabel(status: RestoreOperationInfo["status"]): string {
  switch (status) {
    case "completed": return "已完成";
    case "partial": return "部分完成";
    case "failed": return "失败";
    default: return "进行中";
  }
}

function restoreScopeTitle(operation: RestoreOperationInfo): string {
  switch (operation.scope_type) {
    case "task": return "整任务恢复";
    case "directory":
      return operation.scope_path ? `目录恢复 · ${operation.scope_path}` : "目录恢复";
    case "file": {
      const name = operation.scope_path?.split("/").pop() ?? "";
      return name ? `单文件恢复 · ${name}` : "单文件恢复";
    }
    default: return "恢复操作";
  }
}

function restoreSummary(op: RestoreOperationInfo): string {
  const time = new Date(op.started_at).toLocaleTimeString("zh-CN", {
    hour: "2-digit", minute: "2-digit", hour12: false,
  });
  const source = op.triggered_by === "cli" ? "CLI" : "UI";
  const total = op.requested_count;
  const ok = op.restored_count + op.deleted_count;
  let stat = `${ok}/${total} 个文件已恢复`;
  if (op.failed_count > 0) stat += ` · ${op.failed_count} 失败`;
  return `今天 ${time} · ${source} · ${stat}`;
}

interface Props {
  operations: RestoreOperationInfo[];
  onClose: () => void;
}

export default function RestoreHistoryModal({ operations, onClose }: Props) {
  // Close on overlay click
  const handleOverlayClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.target === e.currentTarget) onClose();
  };

  // Close on ESC
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
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
      <div className="bg-white rounded-xl shadow-[0_20px_60px_rgba(0,0,0,0.2),0_0_0_0.5px_rgba(0,0,0,0.1)] w-[420px] max-h-[480px] flex flex-col overflow-hidden">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-border flex-shrink-0">
          <span className="text-[14px] font-semibold text-t-1">恢复历史</span>
          <button
            onClick={onClose}
            className="w-[22px] h-[22px] rounded-full bg-bg-active text-t-2 flex items-center justify-center text-[11px] hover:bg-border cursor-default"
          >
            <X className="w-3 h-3" />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto">
          {operations.length === 0 ? (
            <div className="py-10 text-center">
              <Clock className="w-8 h-8 text-t-4 mx-auto mb-3" />
              <div className="text-[13px] text-t-2 font-medium mb-1">暂无恢复记录</div>
              <div className="text-[11px] text-t-4 leading-relaxed">
                对文件执行"读档"操作后，<br />恢复记录会出现在这里。
              </div>
            </div>
          ) : (
            operations.map((op) => {
              const StatusIcon = op.status === "completed" ? CheckCircle
                : op.status === "partial" ? AlertTriangle
                : op.status === "failed" ? AlertTriangle
                : Clock;
              const iconBg = op.status === "completed" ? "bg-sys-green/8"
                : op.status === "partial" ? "bg-sys-amber/8"
                : op.status === "failed" ? "bg-sys-red/8"
                : "bg-sys-blue/8";
              const iconColor = op.status === "completed" ? "text-sys-green"
                : op.status === "partial" ? "text-sys-amber"
                : op.status === "failed" ? "text-sys-red"
                : "text-sys-blue";
              const statusColor = iconColor;

              return (
                <div
                  key={op.id}
                  className="flex items-center gap-3 px-5 py-3 border-b border-border-light last:border-b-0 hover:bg-bg-hover transition-colors"
                >
                  <div className={`w-7 h-7 rounded-[7px] flex items-center justify-center flex-shrink-0 ${iconBg} ${iconColor}`}>
                    <StatusIcon className="w-3.5 h-3.5" />
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="text-[12px] font-medium text-t-1 truncate">
                      {restoreScopeTitle(op)}
                    </div>
                    <div className="text-[11px] text-t-3 mt-0.5 truncate">
                      {restoreSummary(op)}
                    </div>
                  </div>
                  <div className={`text-[11px] flex-shrink-0 ${statusColor}`}>
                    {restoreStatusLabel(op.status)}
                  </div>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
