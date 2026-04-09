import { useState } from "react";
import { completeSetup } from "../lib/tauri";
import { invoke } from "@tauri-apps/api/core";

interface DirOption {
  path: string;
  label: string;
  description: string;
  checked: boolean;
}

interface Props {
  onComplete: () => void;
}

export default function SetupWizard({ onComplete }: Props) {

  const [dirs, setDirs] = useState<DirOption[]>([
    {
      path: "",
      label: "桌面",
      description: "Desktop — 常用文件、截图、临时存放区",
      checked: false,
    },
    {
      path: "",
      label: "文稿",
      description: "Documents — 文档、项目、个人资料",
      checked: false,
    },
    {
      path: "",
      label: "下载",
      description: "Downloads — 下载的文件和安装包",
      checked: false,
    },
  ]);
  const [resolved, setResolved] = useState(false);
  const [customDirs, setCustomDirs] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Resolve home dir once on mount
  useState(() => {
    invoke<string>("get_home_dir")
      .then((home) => {
        setDirs([
          {
            path: `${home}/Desktop`,
            label: "桌面",
            description: "Desktop — 常用文件、截图、临时存放区",
            checked: false,
          },
          {
            path: `${home}/Documents`,
            label: "文稿",
            description: "Documents — 文档、项目、个人资料",
            checked: false,
          },
          {
            path: `${home}/Downloads`,
            label: "下载",
            description: "Downloads — 下载的文件和安装包",
            checked: false,
          },
        ]);
        setResolved(true);
      })
      .catch(() => {
        // Fallback — use tilde paths
        setDirs([
          {
            path: "~/Desktop",
            label: "桌面",
            description: "Desktop — 常用文件、截图、临时存放区",
            checked: false,
          },
          {
            path: "~/Documents",
            label: "文稿",
            description: "Documents — 文档、项目、个人资料",
            checked: false,
          },
          {
            path: "~/Downloads",
            label: "下载",
            description: "Downloads — 下载的文件和安装包",
            checked: false,
          },
        ]);
        setResolved(true);
      });
  });

  const toggle = (i: number) => {
    const next = [...dirs];
    next[i] = { ...next[i], checked: !next[i].checked };
    setDirs(next);
  };

  const pickCustomDir = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ directory: true, multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      setCustomDirs((prev) => {
        const merged = [...prev];
        for (const p of paths) {
          if (!merged.includes(p)) merged.push(p);
        }
        return merged;
      });
    } catch (e) {
      console.error("Failed to open dir picker:", e);
    }
  };

  const removeCustom = (p: string) => {
    setCustomDirs((prev) => prev.filter((x) => x !== p));
  };

  const handleStart = async () => {
    const selected = [
      ...dirs.filter((d) => d.checked).map((d) => d.path),
      ...customDirs,
    ];
    if (selected.length === 0) {
      setError("请至少选择一个保护目录");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      await completeSetup(selected);
      onComplete();
    } catch (e) {
      console.error("Setup failed:", e);
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const selectedCount =
    dirs.filter((d) => d.checked).length + customDirs.length;

  return (
    <div className="flex items-center justify-center h-screen bg-gradient-to-br from-surface-secondary to-white">
      <div className="bg-white rounded-2xl shadow-panel border border-surface-border p-8 w-[440px]">
        {/* Header */}
        <div className="text-center mb-7">
          <div className="text-4xl mb-3">🛡️</div>
          <h1 className="text-[17px] font-semibold text-ink mb-1">
            欢迎使用 rew
          </h1>
          <p className="text-[13px] text-ink-muted leading-relaxed">
            选择要保护的目录，rew 会自动记录文件变更，
            <br />
            让你随时可以回到任何历史状态。
          </p>
        </div>

        {/* Recommended dirs */}
        <div className="mb-4">
          <div className="text-[11px] font-semibold text-ink-muted uppercase tracking-wider mb-2">
            推荐保护目录
          </div>
          <div className="space-y-1.5">
            {dirs.map((d, i) => (
              <label
                key={i}
                className={`flex items-start gap-3 px-3 py-2.5 rounded-lg cursor-pointer transition-colors border ${
                  d.checked
                    ? "bg-st-blue/5 border-st-blue/30"
                    : "border-transparent hover:bg-surface-secondary"
                }`}
              >
                <input
                  type="checkbox"
                  checked={d.checked}
                  onChange={() => toggle(i)}
                  className="w-4 h-4 rounded accent-st-blue mt-0.5 flex-shrink-0"
                />
                <div className="flex-1 min-w-0">
                  <div className="text-[13px] font-medium text-ink">
                    {d.label}
                  </div>
                  <div className="text-[11px] text-ink-muted mt-0.5">
                    {d.description}
                  </div>
                </div>
              </label>
            ))}
          </div>
        </div>

        {/* Custom dirs */}
        {customDirs.length > 0 && (
          <div className="mb-3">
            <div className="text-[11px] font-semibold text-ink-muted uppercase tracking-wider mb-1.5">
              自定义目录
            </div>
            <div className="space-y-1">
              {customDirs.map((p) => (
                <div
                  key={p}
                  className="flex items-center gap-2 px-3 py-1.5 bg-surface-secondary rounded-md"
                >
                  <span className="flex-1 text-[12px] text-ink font-mono truncate">
                    {p}
                  </span>
                  <button
                    onClick={() => removeCustom(p)}
                    className="text-ink-faint hover:text-status-red text-[11px] flex-shrink-0"
                  >
                    移除
                  </button>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Add custom dir button */}
        <button
          onClick={pickCustomDir}
          className="w-full mb-5 py-2 rounded-lg border border-dashed border-surface-border text-[13px] text-ink-muted hover:border-st-blue hover:text-st-blue transition-colors"
        >
          + 选择其他目录
        </button>

        {/* Error */}
        {error && (
          <div className="mb-3 px-3 py-2 bg-status-red-bg text-status-red text-[12px] rounded-lg">
            {error}
          </div>
        )}

        {/* Start button */}
        <button
          onClick={handleStart}
          disabled={loading || !resolved}
          className="w-full py-2.5 rounded-lg bg-st-blue text-white text-[13px] font-medium hover:opacity-90 transition-opacity disabled:opacity-40"
        >
          {loading
            ? "启动中..."
            : selectedCount > 0
              ? `开始保护 (${selectedCount} 个目录)`
              : "请选择至少一个目录"}
        </button>

        <p className="text-center text-[11px] text-ink-faint mt-3">
          之后可以在设置中随时添加或移除保护目录
        </p>
      </div>
    </div>
  );
}
