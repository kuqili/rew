import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { listen } from "@tauri-apps/api/event";
import { useScanProgress } from "../hooks/useScanProgress";
import {
  analyzeDirectories, getStorageInfo, addWatchDir, removeWatchDir,
  getMonitoringWindow, setMonitoringWindow,
  getDirIgnoreConfig, updateDirIgnoreConfig, listDirContents, rescanWatchDir,
  type FullAnalysis, type StorageInfo, type DirScanStatus,
  type DirIgnoreConfigInfo, type DirContentItem,
} from "../lib/tauri";

function fmt(b: number): string {
  if (b === 0) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(b) / Math.log(1024));
  return (b / Math.pow(1024, i)).toFixed(i > 1 ? 1 : 0) + " " + u[i];
}

interface Props { onClose: () => void; }

const WINDOW_OPTIONS: { label: string; secs: number }[] = [
  { label: "每 5 分钟存档",  secs: 300 },
  { label: "每 10 分钟存档", secs: 600 },
  { label: "每 30 分钟存档", secs: 1800 },
  { label: "每 1 小时存档",  secs: 3600 },
];

export default function SettingsPanel({ onClose }: Props) {
  const [tab, setTab] = useState<"dirs" | "record" | "about">("dirs");
  const [analysis, setAnalysis] = useState<FullAnalysis | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [expandedDir, setExpandedDir] = useState<string | null>(null);
  const [windowSecs, setWindowSecs] = useState<number>(600);
  const [savingWindow, setSavingWindow] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  const [addingDir, setAddingDir] = useState(false);
  const [addSuccess, setAddSuccess] = useState<string | null>(null);
  const scanProgress = useScanProgress();

  // Merge analysis dirs (persistent) with scanProgress dirs (live status).
  // analysis?.dirs is always populated after the first analyzeDirectories() call
  // and is the source of truth for which directories are being watched.
  // scanProgress?.dirs overlays real-time scan progress on top.
  const allDirs = useMemo((): DirScanStatus[] => {
    const spDirs = scanProgress?.dirs ?? [];
    const aDirs = analysis?.dirs ?? [];
    // Union of all paths, analysis is canonical for membership
    const paths = Array.from(
      new Set([...aDirs.map((d) => d.path), ...spDirs.map((d) => d.path)])
    );
    return paths.map((path) => {
      const sp = spDirs.find((d) => d.path === path);
      if (sp) return sp; // live status takes priority during scan
      // Synthesise a "complete" status from analysis data
      const name = path.split("/").filter(Boolean).pop() ?? path;
      return {
        path,
        name,
        status: "complete" as const,
        files_total: aDirs.find((d) => d.path === path)?.total_files ?? 0,
        files_done: aDirs.find((d) => d.path === path)?.total_files ?? 0,
        percent: 100,
        last_completed_at: null,
      };
    });
  }, [scanProgress?.dirs, analysis?.dirs]);

  const refreshAnalysis = useCallback(() => {
    setAnalyzing(true);
    analyzeDirectories()
      .then(setAnalysis)
      .catch(console.error)
      .finally(() => setAnalyzing(false));
    getStorageInfo().then(setStorage).catch(() => {});
  }, []);

  useEffect(() => {
    refreshAnalysis();
    getMonitoringWindow().then(setWindowSecs).catch(() => {});
    // Poll storage every 3s
    const storageTimer = setInterval(() => {
      getStorageInfo().then(setStorage).catch(() => {});
    }, 3000);
    // Listen for scan-complete to refresh analysis numbers
    const unlistenComplete = listen("scan-complete", () => {
      refreshAnalysis();
    });
    return () => {
      clearInterval(storageTimer);
      unlistenComplete.then((fn) => fn());
    };
  }, [refreshAnalysis]);

  // Also refresh when scanProgress transitions from scanning → not scanning
  const wasScanningRef = useRef(false);
  useEffect(() => {
    const isNowScanning = scanProgress?.is_scanning ?? false;
    if (wasScanningRef.current && !isNowScanning) {
      // Scan just completed — refresh analysis
      refreshAnalysis();
    }
    wasScanningRef.current = isNowScanning;
  }, [scanProgress?.is_scanning, refreshAnalysis]);

  const handleAddDir = async () => {
    setAddError(null);
    setAddSuccess(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const s = await open({ directory: true, multiple: false });
      if (!s) return;
      setAddingDir(true);
      await addWatchDir(s as string);
      const name = (s as string).split("/").pop() || s as string;
      setAddSuccess(`已添加「${name}」，正在扫描…`);
      setTimeout(() => setAddSuccess(null), 4000);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setAddError(msg);
    } finally {
      setAddingDir(false);
    }
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      <div className="flex-shrink-0 bg-surface-secondary border-b border-surface-border px-6 py-4 flex items-center justify-between">
        <h2 className="text-[15px] font-semibold text-ink">设置</h2>
        <button onClick={onClose} className="text-ink-muted hover:text-ink text-lg">×</button>
      </div>
      <div className="flex-shrink-0 border-b border-surface-border px-6 flex">
        {([
          { k: "dirs" as const,   l: "目录管理" },
          { k: "record" as const, l: "存档设置" },
          { k: "about" as const,  l: "关于" },
        ]).map((t) => (
          <button key={t.k} onClick={() => setTab(t.k)}
            className={`px-4 py-2.5 text-[13px] border-b-2 transition-colors ${tab === t.k ? "border-st-blue text-st-blue font-medium" : "border-transparent text-ink-secondary hover:text-ink"}`}>
            {t.l}
          </button>
        ))}
      </div>
      <div className="flex-1 overflow-y-auto px-6 py-5">
        {tab === "record" ? (
          <div className="max-w-[520px] space-y-6">
            <div>
              <h3 className="text-[13px] font-semibold text-ink mb-1">自动存档频率</h3>
              <p className="text-2xs text-ink-muted leading-relaxed mb-3">
                每隔固定时间自动创建一个存档点，记录该时间段内所有文件的净变更（相同文件反复修改只算一次）。
                读档即可回到存档点之前的文件状态。间隔越长，存档条数越少，但每条覆盖的变更越多。
              </p>
              <div className="flex gap-2 flex-wrap">
                {WINDOW_OPTIONS.map((opt) => (
                  <button
                    key={opt.secs}
                    onClick={async () => {
                      setSavingWindow(true);
                      try {
                        await setMonitoringWindow(opt.secs);
                        setWindowSecs(opt.secs);
                      } finally {
                        setSavingWindow(false);
                      }
                    }}
                    disabled={savingWindow}
                    className={`px-4 py-2 rounded-lg text-[13px] border transition-colors ${
                      windowSecs === opt.secs
                        ? "bg-st-blue text-white border-st-blue"
                        : "bg-white text-ink-secondary border-surface-border hover:border-st-blue hover:text-st-blue"
                    } disabled:opacity-40`}
                  >
                    {opt.label}
                  </button>
                ))}
              </div>
              <p className="mt-3 text-2xs text-ink-muted">
                当前设置：每 {windowSecs >= 3600 ? `${windowSecs / 3600} 小时` : `${windowSecs / 60} 分钟`} 自动存档一次
                {savingWindow && " — 保存中..."}
              </p>

              {/* Behavior notes */}
              <div className="mt-4 space-y-2">
                <InfoNote>
                  <b>更改频率后</b>，当前正在积累的存档周期会<b>立即封存</b>，新频率从下一个周期开始生效。
                </InfoNote>
                <InfoNote>
                  <b>AI 任务优先</b>：当 AI 工具（Cursor / Claude Code）正在运行时，文件变更会计入 AI 任务记录，
                  不会同时出现在定时存档中，避免重复。AI 任务完成后，定时存档自动继续。
                </InfoNote>
              </div>
            </div>
          </div>
        ) : tab === "dirs" ? (
          <div className="space-y-6 max-w-[680px]">
            {/* Global overview */}
            <div className="bg-surface-secondary rounded-lg px-4 py-3 border border-surface-border/60">
              {analyzing ? (
                <div className="flex items-center gap-2 text-[13px] text-ink-secondary">
                  <span className="inline-block animate-spin">◐</span> 正在分析保护目录...
                </div>
              ) : analysis ? (
                <div className="space-y-3">
                  <div>
                    <div className="text-2xs text-ink-muted mb-0.5">保护范围</div>
                    <div className="flex items-baseline gap-2">
                      <span className="text-[17px] font-semibold text-ink">{analysis.total_files.toLocaleString()}</span>
                      <span className="text-[13px] text-ink-muted">个文件</span>
                      <span className="text-[13px] text-ink-secondary">· {fmt(analysis.total_bytes)}</span>
                    </div>
                    <div className="flex flex-wrap gap-x-4 gap-y-0.5 text-2xs text-ink-muted mt-1">
                      {analysis.dirs.map((d) => (
                        <span key={d.path}>{d.path.split("/").pop()}: {fmt(d.total_bytes)}</span>
                      ))}
                    </div>
                  </div>
                  {(() => {
                    const isCurrentlyScanning = scanProgress?.is_scanning ?? false;
                    const totalFailed = allDirs.reduce((s, d) => s + (d.files_failed ?? 0), 0);
                    const totalDone = allDirs.reduce((s, d) => s + d.files_done, 0);
                    const totalFiles = allDirs.reduce((s, d) => s + d.files_total, 0);
                    const allComplete = !isCurrentlyScanning &&
                      allDirs.length > 0 &&
                      allDirs.every((d) => d.status === "complete");

                    if (isCurrentlyScanning) {
                      const pct = totalFiles > 0 ? Math.min((totalDone / totalFiles) * 100, 100) : 0;
                      return (
                        <div>
                          <div className="text-2xs text-ink-muted mb-0.5">备份进度</div>
                          <div className="flex items-baseline gap-2 mb-1.5">
                            <span className="text-[15px] font-semibold text-st-blue">{totalDone.toLocaleString()}</span>
                            <span className="text-[13px] text-ink-muted">/ {totalFiles.toLocaleString()} 个文件已备份</span>
                          </div>
                          <div className="w-full h-1.5 bg-surface-border rounded-full overflow-hidden">
                            <div className="h-full bg-st-blue rounded-full transition-all duration-300"
                              style={{ width: `${pct}%` }} />
                          </div>
                        </div>
                      );
                    }

                    if (allComplete && totalFailed > 0) {
                      // Partial success — guide user to rescan
                      return (
                        <div className="space-y-2">
                          <div className="flex items-baseline gap-2">
                            <span className="text-[15px] font-semibold text-status-yellow">⚠ 部分文件未能备份</span>
                          </div>
                          <div className="text-[12px] text-ink-muted leading-relaxed">
                            共 {analysis.total_files.toLocaleString()} 个文件，其中
                            <span className="text-status-yellow font-medium"> {totalFailed.toLocaleString()} 个备份失败</span>
                            （通常是权限不足或文件被锁定），点击对应目录重新扫描。
                          </div>
                          <div className="flex flex-wrap gap-2 pt-0.5">
                            {allDirs
                              .filter((d) => (d.files_failed ?? 0) > 0)
                              .map((d) => (
                                <button
                                  key={d.path}
                                  onClick={async () => {
                                    try { await rescanWatchDir(d.path); refreshAnalysis(); }
                                    catch (e) { console.error(e); }
                                  }}
                                  className="px-3 py-1.5 rounded-lg border border-status-yellow/40 bg-status-yellow-bg text-status-yellow text-[12px] hover:opacity-80 transition-opacity"
                                >
                                  重新扫描 {d.name}（{(d.files_failed ?? 0).toLocaleString()} 个失败）
                                </button>
                              ))
                            }
                          </div>
                        </div>
                      );
                    }

                    if (allComplete) {
                      return (
                        <div className="flex items-center gap-2">
                          <span className="text-[15px] font-semibold text-status-green">✓ 初始备份已完成</span>
                          <span className="text-[13px] text-ink-muted">{analysis.total_files.toLocaleString()} 个文件受保护</span>
                        </div>
                      );
                    }

                    if (allDirs.length > 0) {
                      return (
                        <div className="flex items-center gap-2 text-[13px] text-ink-muted">
                          <span className="animate-spin inline-block">◐</span>
                          <span>正在启动初始扫描...</span>
                        </div>
                      );
                    }

                    return null;
                  })()}
                  {storage && (
                    <div className="text-2xs text-ink-muted leading-relaxed px-1">
                      使用 APFS clonefile 技术备份，与原文件共享磁盘空间，不产生额外占用。
                    </div>
                  )}
                </div>
              ) : null}
            </div>

            {/* Directory list */}
            <div>
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-[13px] font-semibold text-ink">保护目录</h3>
                <button
                  onClick={handleAddDir}
                  disabled={addingDir}
                  className="px-3 py-1 rounded-md bg-st-blue text-white text-[13px] font-medium hover:opacity-90 disabled:opacity-50 flex items-center gap-1.5"
                >
                  {addingDir ? (
                    <><span className="animate-spin text-[11px]">◐</span><span>添加中…</span></>
                  ) : (
                    <span>+ 添加目录</span>
                  )}
                </button>
              </div>

              {addSuccess && (
                <div className="mb-2 px-3 py-2 bg-status-green-bg text-status-green text-[12px] rounded-lg leading-relaxed flex items-center gap-2">
                  <span>✓</span><span>{addSuccess}</span>
                </div>
              )}
              {addError && (
                <div className="mb-2 px-3 py-2 bg-status-red-bg text-status-red text-[12px] rounded-lg leading-relaxed">
                  {addError}
                </div>
              )}

              <div className="space-y-2">
                {allDirs.map((dir) => (
                  <DirCard
                    key={dir.path}
                    dir={dir}
                    da={analysis?.dirs.find((d) => d.path === dir.path) || null}
                    expanded={expandedDir === dir.path}
                    onToggle={() => setExpandedDir(expandedDir === dir.path ? null : dir.path)}
                    onRemove={async () => {
                      if (confirm(`停止保护「${dir.name}」？\n已备份的文件不会被删除。`)) {
                        await removeWatchDir(dir.path);
                        refreshAnalysis();
                        setAddError(null);
                      }
                    }}
                  />
                ))}
              </div>

              {allDirs.length === 0 && (
                <div className="text-center py-8 text-ink-muted text-[13px]">
                  还没有保护目录，点击「+ 添加目录」开始保护
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="space-y-4 max-w-[640px]">
            <h3 className="text-[13px] font-semibold text-ink">rew — AI 时代的文件安全网</h3>
            <p className="text-2xs text-ink-secondary leading-relaxed">
              实时监控文件，AI 工具操作时自动备份。误删或改错，一键撤销。
            </p>
            <div className="text-2xs text-ink-muted space-y-1">
              <div>版本: 0.1.0</div>
              <div>存储: APFS clonefile (CoW)</div>
              {storage && <div>备份: {storage.object_count.toLocaleString()} 对象 · {fmt(storage.apparent_bytes)}</div>}
            </div>
            <div className="mt-4 pt-3 border-t border-surface-border">
              <h4 className="text-2xs font-semibold text-ink mb-2">默认不备份的文件类型</h4>
              <div className="text-2xs text-ink-muted space-y-1 leading-relaxed">
                <div>• <b>应用程序</b> — .app 包内文件</div>
                <div>• <b>安装包</b> — .dmg, .pkg, .iso, .msi, .exe</div>
                <div>• <b>开发产物</b> — node_modules, .git, target, __pycache__</div>
                <div>• <b>系统临时文件</b> — .DS_Store, Thumbs.db, .swp</div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ─── Directory Card ───

function DirCard({
  dir, da, expanded, onToggle, onRemove,
}: {
  dir: DirScanStatus;
  da: { total_files: number; total_bytes: number; categories: { category: string; total_bytes: number; file_count: number }[] } | null;
  expanded: boolean;
  onToggle: () => void;
  onRemove: () => Promise<void>;
}) {
  return (
    <div className="border border-surface-border rounded-lg overflow-hidden">
      <button onClick={onToggle} className="w-full text-left px-4 py-3 hover:bg-surface-hover/50 transition-colors">
        <div className="flex items-center justify-between mb-0.5">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-medium text-ink">{dir.name}</span>
            {dir.status === "complete" && <span className="badge bg-status-green-bg text-status-green">已保护</span>}
            {dir.status === "scanning" && <span className="badge bg-status-yellow-bg text-status-yellow">扫描中 {dir.percent.toFixed(0)}%</span>}
            {dir.status === "pending" && <span className="badge bg-surface-secondary text-ink-muted">等待</span>}
          </div>
          <span className="text-2xs text-ink-muted">{expanded ? "▾" : "▸"}</span>
        </div>
        <div className="text-2xs text-ink-muted">{dir.path}</div>
        {da && (
          <div className="text-2xs text-ink-secondary mt-1">
            {da.total_files.toLocaleString()} 个文件 · {fmt(da.total_bytes)}
          </div>
        )}
        {dir.status === "scanning" && (
          <div className="mt-2 w-full bg-surface-border rounded-full h-1 overflow-hidden">
            <div className="h-full bg-st-blue rounded-full transition-all duration-300" style={{ width: `${Math.min(dir.percent, 100)}%` }} />
          </div>
        )}
      </button>

      {expanded && (
        <div className="border-t border-surface-border">
          {/* File type distribution */}
          {da && da.categories.length > 0 && (
            <div className="bg-surface-secondary/50 px-4 py-3">
              <div className="text-2xs font-medium text-ink-secondary mb-2">文件类型分布</div>
              <div className="space-y-1.5">
                {da.categories.map((c) => {
                  const pct = da.total_bytes > 0 ? (c.total_bytes / da.total_bytes) * 100 : 0;
                  return (
                    <div key={c.category} className="flex items-center gap-2 text-2xs">
                      <div className="w-[70px] truncate text-ink-secondary">{c.category}</div>
                      <div className="flex-1 h-1.5 bg-surface-border rounded-full overflow-hidden">
                        <div className="h-full rounded-full bg-st-blue" style={{ width: `${Math.max(pct, 1)}%` }} />
                      </div>
                      <div className="w-[55px] text-right tabular-nums text-ink-muted">{fmt(c.total_bytes)}</div>
                      <div className="w-[45px] text-right tabular-nums text-ink-muted">{c.file_count} 个</div>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* Filter config — sub-dirs and extensions */}
          <DirFilterSection dirPath={dir.path} scanStatus={dir.status} />

          {/* Remove */}
          <RemoveDirButton dir={dir} onRemove={onRemove} />
        </div>
      )}
    </div>
  );
}

// ─── Remove Button with loading/confirm state ───

function RemoveDirButton({ dir, onRemove }: { dir: DirScanStatus; onRemove: () => Promise<void> }) {
  const [removing, setRemoving] = useState(false);
  const [confirm, setConfirm] = useState(false);

  const handleClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirm) { setConfirm(true); return; }
    setRemoving(true);
    try {
      await onRemove();
    } finally {
      setRemoving(false);
      setConfirm(false);
    }
  };

  return (
    <div className="px-4 py-2.5 border-t border-surface-border/60 bg-surface-secondary/30 flex items-center gap-3">
      <button
        onClick={handleClick}
        disabled={removing || dir.status === "scanning"}
        className={`text-2xs transition-colors disabled:opacity-30 ${
          confirm
            ? "text-status-red font-medium"
            : "text-ink-faint hover:text-status-red"
        }`}
      >
        {removing ? (
          <span className="flex items-center gap-1">
            <span className="animate-spin">◐</span> 移除中…
          </span>
        ) : confirm ? (
          "确认移除（点击确认）"
        ) : (
          "移除此目录"
        )}
      </button>
      {confirm && !removing && (
        <button
          onClick={(e) => { e.stopPropagation(); setConfirm(false); }}
          className="text-2xs text-ink-faint hover:text-ink transition-colors"
        >
          取消
        </button>
      )}
    </div>
  );
}

// ─── Filter Section inside DirCard ───

function DirFilterSection({ dirPath, scanStatus }: { dirPath: string; scanStatus: string }) {
  const [config, setConfig] = useState<DirIgnoreConfigInfo | null>(null);
  const [extInput, setExtInput] = useState("");
  const [saved, setSaved] = useState(false);
  const [rescanning, setRescanning] = useState(false);
  const [rescanDone, setRescanDone] = useState(false);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    getDirIgnoreConfig(dirPath).then(setConfig).catch(console.error);
  }, [dirPath]);

  // When scan transitions to complete while we initiated a rescan, show toast
  useEffect(() => {
    if (scanStatus === "complete" && rescanning) {
      setRescanning(false);
      setRescanDone(true);
      setTimeout(() => setRescanDone(false), 4000);
    }
  }, [scanStatus, rescanning]);

  const save = useCallback((updated: DirIgnoreConfigInfo) => {
    setConfig(updated);
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      updateDirIgnoreConfig(dirPath, updated.exclude_dirs, updated.exclude_extensions)
        .then(() => { setSaved(true); setTimeout(() => setSaved(false), 2000); })
        .catch(console.error);
    }, 300);
  }, [dirPath]);

  const handleRescan = async () => {
    // Flush any pending config save before rescanning
    if (saveTimer.current) {
      clearTimeout(saveTimer.current);
      saveTimer.current = null;
      if (config) {
        await updateDirIgnoreConfig(dirPath, config.exclude_dirs, config.exclude_extensions).catch(console.error);
      }
    }
    setRescanning(true);
    setRescanDone(false);
    try { await rescanWatchDir(dirPath); }
    catch (e) { console.error("Rescan failed:", e); setRescanning(false); }
  };

  if (!config) return null;

  /** Toggle a relative path in exclude_dirs. */
  const toggleExcludePath = (relPath: string) => {
    const excluded = config.exclude_dirs.includes(relPath);
    const newDirs = excluded
      ? config.exclude_dirs.filter((d) => d !== relPath)
      : [...config.exclude_dirs, relPath];
    save({ ...config, exclude_dirs: newDirs });
  };

  const addExt = () => {
    const val = extInput.trim().replace(/^\./, "");
    if (!val || config.exclude_extensions.includes(val)) return;
    save({ ...config, exclude_extensions: [...config.exclude_extensions, val] });
    setExtInput("");
  };

  const removeExt = (e: string) => {
    save({ ...config, exclude_extensions: config.exclude_extensions.filter((x) => x !== e) });
  };

  const isScanning = scanStatus === "scanning" || rescanning;

  return (
    <div className="px-4 py-3 border-t border-surface-border/60 bg-white">
      {/* Header */}
      <div className="flex items-center justify-between mb-2">
        <div className="text-2xs font-medium text-ink-secondary">
          过滤配置
          {config.exclude_dirs.length > 0 && (
            <span className="ml-1.5 text-[10px] text-ink-faint font-normal">
              {config.exclude_dirs.length} 项已排除
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {saved && <span className="text-[10px] text-status-green">✓ 配置已保存</span>}
          {rescanDone && <span className="text-[10px] text-status-green">✓ 扫描完成</span>}
          {isScanning ? (
            <span className="text-[10px] text-st-blue flex items-center gap-1">
              <span className="animate-spin inline-block">◐</span>
              {rescanning ? "重新扫描中…" : "扫描中…"}
            </span>
          ) : (
            <button
              onClick={handleRescan}
              className="text-[10px] text-ink-muted hover:text-st-blue transition-colors"
              title="以当前过滤配置重新扫描此目录，更新保护统计"
            >
              重新扫描
            </button>
          )}
        </div>
      </div>

      <div className="mb-2.5 text-[10px] text-ink-muted leading-relaxed">
        勾选后该路径及其所有子内容将被排除，不记录、不存档。规则从<b>下次变更</b>起生效。
        「重新扫描」可更新保护文件统计，但<b>不会删除</b>已有的历史存档记录。
      </div>

      {/* Tree picker for exclude dirs/files */}
      <div className="mb-3">
        <div className="text-[11px] text-ink-muted mb-1">排除目录或文件</div>
        <div className="max-h-[200px] overflow-y-auto border border-surface-border/60 rounded bg-surface-secondary/30">
          <ExcludeTreeRoot
            watchDir={dirPath}
            excludeDirs={config.exclude_dirs}
            onToggle={toggleExcludePath}
          />
        </div>
      </div>

      {/* Exclude extensions */}
      <div>
        <div className="text-[11px] text-ink-muted mb-1.5">排除文件类型（任意层级）</div>
        <div className="flex flex-wrap gap-1 mb-1.5">
          {config.exclude_extensions.map((e) => (
            <span
              key={e}
              className="inline-flex items-center gap-0.5 bg-surface-secondary rounded px-1.5 py-0.5 text-[11px] text-ink-secondary"
            >
              .{e}
              <button
                onClick={() => removeExt(e)}
                className="text-ink-faint hover:text-status-red ml-0.5 leading-none"
              >
                ×
              </button>
            </span>
          ))}
        </div>
        <div className="flex gap-1.5">
          <input
            value={extInput}
            onChange={(e) => setExtInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addExt()}
            placeholder="如 log、tmp、DS_Store"
            className="flex-1 min-w-0 px-2 py-1 bg-surface-secondary border border-surface-border rounded text-[11px] focus:outline-none focus:border-st-blue"
          />
          <button onClick={addExt} className="px-2 py-1 bg-st-blue text-white rounded text-[11px] hover:opacity-90">+</button>
        </div>
      </div>
    </div>
  );
}

// ─── Recursive exclude tree ───

/** Root of the exclude tree — loads the first level of `watchDir`. */
function ExcludeTreeRoot({
  watchDir,
  excludeDirs,
  onToggle,
}: {
  watchDir: string;
  excludeDirs: string[];
  onToggle: (relPath: string) => void;
}) {
  const [children, setChildren] = useState<DirContentItem[] | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    listDirContents(watchDir)
      .then(setChildren)
      .catch(() => setChildren([]))
      .finally(() => setLoading(false));
  }, [watchDir]);

  if (loading) {
    return (
      <div className="px-3 py-2 text-[10px] text-ink-faint flex items-center gap-1">
        <span className="animate-spin">◐</span> 加载中…
      </div>
    );
  }
  if (!children || children.length === 0) {
    return <div className="px-3 py-2 text-[10px] text-ink-faint">（无内容）</div>;
  }

  return (
    <div>
      {children.map((child) => {
        const relPath = child.name + (child.is_dir ? "" : "");
        return (
          <ExcludeTreeNode
            key={child.full_path}
            item={child}
            relPath={relPath}
            watchDir={watchDir}
            excludeDirs={excludeDirs}
            onToggle={onToggle}
            depth={0}
          />
        );
      })}
    </div>
  );
}

/** One node in the exclude tree. */
function ExcludeTreeNode({
  item,
  relPath,
  watchDir,
  excludeDirs,
  onToggle,
  depth,
}: {
  item: DirContentItem;
  relPath: string;
  watchDir: string;
  excludeDirs: string[];
  onToggle: (relPath: string) => void;
  depth: number;
}) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<DirContentItem[] | null>(null);
  const [loading, setLoading] = useState(false);

  const isChecked = excludeDirs.some(
    (e) => e === relPath || relPath.startsWith(e + "/") || relPath.startsWith(e)
  );
  // Direct match (this path is in exclude list)
  const isDirectlyExcluded = excludeDirs.includes(relPath);
  // Parent is excluded
  const isParentExcluded = !isDirectlyExcluded && isChecked;

  const handleToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!item.is_dir) return;
    const willExpand = !expanded;
    if (willExpand && children === null) {
      setLoading(true);
      try {
        const items = await listDirContents(item.full_path);
        setChildren(items);
      } catch { setChildren([]); }
      finally { setLoading(false); }
    }
    setExpanded(willExpand);
  };

  const indent = depth * 14;

  return (
    <div>
      <div
        className={`flex items-center gap-1.5 py-0.5 pr-2 text-[11px] transition-colors ${
          isParentExcluded ? "opacity-40" : "hover:bg-surface-hover/60"
        }`}
        style={{ paddingLeft: `${8 + indent}px` }}
      >
        <input
          type="checkbox"
          checked={isDirectlyExcluded}
          disabled={isParentExcluded}
          onChange={() => onToggle(relPath)}
          className="accent-st-blue w-3 h-3 flex-shrink-0 cursor-pointer disabled:cursor-not-allowed"
        />
        {/* Expand toggle for dirs */}
        {item.is_dir ? (
          <button
            onClick={handleToggle}
            className="w-4 h-3.5 flex items-center justify-center text-[9px] text-ink-faint hover:text-ink-secondary flex-shrink-0"
          >
            {loading ? <span className="animate-spin">◐</span> : expanded ? "▾" : "▸"}
          </button>
        ) : (
          <span className="w-4 flex-shrink-0 text-center text-[10px] opacity-50">
            {item.name.includes(".") ? "·" : "·"}
          </span>
        )}
        <span
          className={`truncate flex-1 cursor-pointer select-none ${
            isDirectlyExcluded ? "line-through text-ink-faint" : "text-ink-secondary"
          }`}
          onClick={() => onToggle(relPath)}
          title={`${watchDir}/${relPath}`}
        >
          {item.is_dir ? `${item.name}/` : item.name}
        </span>
      </div>

      {/* Children */}
      {item.is_dir && expanded && children !== null && (
        <div className="border-l border-surface-border/30" style={{ marginLeft: `${20 + indent}px` }}>
          {children.length === 0 ? (
            <div className="px-3 py-0.5 text-[10px] text-ink-faint">（空）</div>
          ) : (
            children.map((child) => {
              const childRel = `${relPath}/${child.name}`;
              return (
                <ExcludeTreeNode
                  key={child.full_path}
                  item={child}
                  relPath={childRel}
                  watchDir={watchDir}
                  excludeDirs={excludeDirs}
                  onToggle={onToggle}
                  depth={depth + 1}
                />
              );
            })
          )}
        </div>
      )}
    </div>
  );
}

// ─── Shared helper ───
function InfoNote({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex gap-2 bg-surface-secondary border border-surface-border/60 rounded-lg px-3 py-2 text-[11px] text-ink-secondary leading-relaxed">
      <span className="text-ink-faint flex-shrink-0 mt-px">ℹ</span>
      <span>{children}</span>
    </div>
  );
}
