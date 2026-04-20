import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { listen } from "@tauri-apps/api/event";
import { X, Plus, FolderOpen, RefreshCw, Trash2, ChevronDown, ChevronRight, Info, CheckCircle, Settings, Clock, Cpu, HelpCircle } from "lucide-react";
import { useScanProgress } from "../hooks/useScanProgress";
import { getToolBrandIcon } from "./ToolIcons";
import { useUpdater } from "../hooks/useUpdater";
import {
  analyzeDirectories, getStorageInfo, getDirStats, addWatchDir, removeWatchDir,
  getMonitoringWindow, setMonitoringWindow,
  getDirIgnoreConfig, updateDirIgnoreConfig, listDirContents, rescanWatchDir,
  detectAiTools, installToolHook, uninstallToolHook,
  type FullAnalysis, type StorageInfo, type DirScanStatus, type DirStatsResult,
  type DirIgnoreConfigInfo, type DirContentItem, type AiToolInfo,
} from "../lib/tauri";

function fmt(b: number): string {
  if (b === 0) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(b) / Math.log(1024));
  return (b / Math.pow(1024, i)).toFixed(i > 1 ? 1 : 0) + " " + u[i];
}

interface Props {
  onClose: () => void;
  initialTab?: "dirs" | "record" | "ai_tools" | "about";
}

const WINDOW_OPTIONS: { label: string; secs: number }[] = [
  { label: "5 分钟",  secs: 300 },
  { label: "10 分钟", secs: 600 },
  { label: "30 分钟", secs: 1800 },
  { label: "1 小时",  secs: 3600 },
];

const TABS = [
  { k: "dirs" as const,     label: "保护目录", icon: <FolderOpen className="w-4 h-4" /> },
  { k: "record" as const,   label: "自动存档", icon: <Clock className="w-4 h-4" /> },
  { k: "ai_tools" as const, label: "AI 工具",  icon: <Cpu className="w-4 h-4" /> },
  { k: "about" as const,    label: "关于",     icon: <HelpCircle className="w-4 h-4" /> },
];

export default function SettingsPanel({ onClose, initialTab = "dirs" }: Props) {
  const [tab, setTab] = useState<"dirs" | "record" | "ai_tools" | "about">(initialTab);
  const [dirStats, setDirStats] = useState<DirStatsResult | null>(null);
  const [analysis, setAnalysis] = useState<FullAnalysis | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const analysisLoadedRef = useRef(false);
  const [storage, setStorage] = useState<StorageInfo | null>(null);
  const [expandedDir, setExpandedDir] = useState<string | null>(null);
  const [windowSecs, setWindowSecs] = useState<number>(600);
  const [savingWindow, setSavingWindow] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  const [addingDir, setAddingDir] = useState(false);
  const [addSuccess, setAddSuccess] = useState<string | null>(null);
  const scanProgress = useScanProgress();

  // Merge sources: dirStats (fast, file counts) + analysis (slow, sizes) + scanProgress (live)
  const allDirs = useMemo((): DirScanStatus[] => {
    const spDirs = scanProgress?.dirs ?? [];
    // Use dirStats paths as base, fallback to analysis dirs
    const statDirs = dirStats?.dirs ?? [];
    const aDirs = analysis?.dirs ?? [];
    const paths = Array.from(
      new Set([
        ...statDirs.map((d) => d.path),
        ...aDirs.map((d) => d.path),
        ...spDirs.map((d) => d.path),
      ])
    );
    return paths.map((path) => {
      const sp = spDirs.find((d) => d.path === path);
      if (sp) return sp;
      const statEntry = statDirs.find((d) => d.path === path);
      const aEntry = aDirs.find((d) => d.path === path);
      const fileCount = statEntry?.file_count ?? aEntry?.total_files ?? 0;
      const name = path.split("/").filter(Boolean).pop() ?? path;
      return {
        path,
        name,
        status: "complete" as const,
        files_total: fileCount,
        files_done: fileCount,
        files_failed: 0,
        percent: 100,
        last_completed_at: null,
      };
    });
  }, [scanProgress?.dirs, dirStats?.dirs, analysis?.dirs]);

  // Fast refresh: file counts from file_index (ms-level)
  const refreshDirStats = useCallback(() => {
    getDirStats().then(setDirStats).catch(console.error);
  }, []);

  // Slow refresh: full analysis with sizes (seconds, runs in background)
  const refreshAnalysis = useCallback(() => {
    setAnalyzing(true);
    analysisLoadedRef.current = true;
    analyzeDirectories()
      .then(setAnalysis)
      .catch(console.error)
      .finally(() => setAnalyzing(false));
    getStorageInfo().then(setStorage).catch(() => {});
  }, []);

  useEffect(() => {
    // Fast path only — file counts appear instantly, sizes on-demand
    refreshDirStats();
    getMonitoringWindow().then(setWindowSecs).catch(() => {});
    const unlistenComplete = listen("scan-complete", () => {
      refreshDirStats();
      // If user had already triggered analysis, refresh it
      if (analysisLoadedRef.current) refreshAnalysis();
    });
    return () => {
      unlistenComplete.then((fn) => fn());
    };
  }, [refreshDirStats, refreshAnalysis]);

  const wasScanningRef = useRef(false);
  useEffect(() => {
    const isNowScanning = scanProgress?.is_scanning ?? false;
    if (wasScanningRef.current && !isNowScanning) {
      refreshDirStats();
      if (analysisLoadedRef.current) refreshAnalysis();
    }
    wasScanningRef.current = isNowScanning;
  }, [scanProgress?.is_scanning, refreshDirStats, refreshAnalysis]);

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

  const tabTitle = TABS.find((t) => t.k === tab)?.label ?? "设置";

  return (
    <div className="h-full w-full flex overflow-hidden">
      {/* Sidebar nav */}
      <div className="w-[180px] flex-shrink-0 bg-bg-sidebar backdrop-blur-[40px] saturate-[180%] border-r border-border/60 flex flex-col py-4">
        <div className="text-[13px] font-semibold text-t-1 px-4 pb-3 tracking-[-0.01em]">设置</div>
        <div className="flex flex-col gap-[1px] px-2">
          {TABS.map((t) => {
            const active = tab === t.k;
            return (
              <button
                key={t.k}
                onClick={() => setTab(t.k)}
                className={`flex items-center gap-2 px-2.5 py-[6px] rounded-md text-[12px] text-left w-full cursor-default transition-colors ${
                  active
                    ? "bg-sys-blue/8 text-sys-blue font-medium"
                    : "text-t-2 hover:bg-bg-hover"
                }`}
              >
                <span className={active ? "text-sys-blue" : "text-t-3"}>{t.icon}</span>
                {t.label}
              </button>
            );
          })}
        </div>
      </div>

      {/* Content area */}
      <div className="flex-1 flex flex-col overflow-hidden">
        {/* Content header */}
        <div className="flex-shrink-0 flex items-center justify-between px-6 pt-4 pb-3 border-b border-border-light">
          <h2 className="text-[15px] font-semibold text-t-1">{tabTitle}</h2>
          <button onClick={onClose} className="w-[22px] h-[22px] rounded-full bg-bg-active text-t-2 flex items-center justify-center text-[11px] hover:bg-border cursor-default">
            <X className="w-3 h-3" />
          </button>
        </div>

        {/* Scrollable content */}
        <div className="flex-1 overflow-y-auto px-6 py-5">
        {tab === "record" ? (
          <div className="space-y-6">
            <div>
              <h3 className="text-[13px] font-semibold text-t-1 mb-1">自动存档频率</h3>
              <p className="text-[11px] text-t-3 leading-relaxed mb-3">
                每隔固定时间自动创建一个存档点，记录该时间段内所有文件的净变更（相同文件反复修改只算一次）。
                读档即可回到存档点之前的文件状态。间隔越长，存档条数越少，但每条覆盖的变更越多。
              </p>

              {/* Segmented control */}
              <div className="inline-flex bg-bg-grouped rounded-md p-0.5 mb-3">
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
                    className={`px-3.5 py-1 rounded text-[12px] font-medium transition-colors
                      ${windowSecs === opt.secs
                        ? 'bg-white text-t-1 shadow-[0_0.5px_1.5px_rgba(0,0,0,0.08),0_0_0_0.5px_rgba(0,0,0,0.04)]'
                        : 'text-t-2 hover:text-t-1'} disabled:opacity-40`}
                  >
                    {opt.label}
                  </button>
                ))}
              </div>

              <p className="text-[11px] text-t-3">
                当前设置：每 {windowSecs >= 3600 ? `${windowSecs / 3600} 小时` : `${windowSecs / 60} 分钟`} 自动存档一次
                {savingWindow && " — 保存中..."}
              </p>

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
          <div className="space-y-6">
            {/* Global overview */}
            <div className="bg-bg-grouped rounded-md px-5 py-4">
              {!dirStats && !analysis ? (
                <div className="flex items-center gap-2 text-[13px] text-t-2">
                  <span className="inline-block animate-spin">◐</span> 正在分析保护目录...
                </div>
              ) : (
                <div className="space-y-3">
                  <div>
                    <div className="text-[11px] text-t-3 mb-0.5">保护范围</div>
                    <div className="flex items-baseline gap-2">
                      <span className="text-[17px] font-semibold text-t-1">{(dirStats?.total_files ?? analysis?.total_files ?? 0).toLocaleString()}</span>
                      <span className="text-[13px] text-t-3">个文件</span>
                      {analysis ? (
                        <span className="text-[13px] text-t-2">· {fmt(analysis.total_bytes)}</span>
                      ) : analyzing ? (
                        <span className="text-[13px] text-t-4">· 计算大小中...</span>
                      ) : (
                        <button
                          onClick={refreshAnalysis}
                          className="text-[12px] text-sys-blue hover:text-sys-blue/80 transition-colors cursor-default"
                        >
                          计算大小
                        </button>
                      )}
                    </div>
                    {analysis && (
                      <div className="flex flex-wrap gap-x-4 gap-y-0.5 text-[11px] text-t-3 mt-1">
                        {analysis.dirs.map((d) => (
                          <span key={d.path}>{d.path.split("/").pop()}: {fmt(d.total_bytes)}</span>
                        ))}
                      </div>
                    )}
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
                          <div className="text-[11px] text-t-3 mb-0.5">备份进度</div>
                          <div className="flex items-baseline gap-2 mb-1.5">
                            <span className="text-[15px] font-semibold text-sys-blue">{totalDone.toLocaleString()}</span>
                            <span className="text-[13px] text-t-3">/ {totalFiles.toLocaleString()} 个文件已备份</span>
                          </div>
                          <div className="w-full h-1.5 bg-border rounded-full overflow-hidden">
                            <div className="h-full bg-sys-blue rounded-full transition-all duration-300"
                              style={{ width: `${pct}%` }} />
                          </div>
                        </div>
                      );
                    }

                    if (allComplete && totalFailed > 0) {
                      return (
                        <div className="space-y-2">
                          <div className="flex items-baseline gap-2">
                            <span className="text-[15px] font-semibold text-sys-amber">⚠ 部分文件未能备份</span>
                          </div>
                          <div className="text-[12px] text-t-3 leading-relaxed">
                            共 {(dirStats?.total_files ?? analysis?.total_files ?? 0).toLocaleString()} 个文件，其中
                            <span className="text-sys-amber font-medium"> {totalFailed.toLocaleString()} 个备份失败</span>
                            （通常是权限不足或文件被锁定），点击对应目录重新扫描。
                          </div>
                          <div className="flex flex-wrap gap-2 pt-0.5">
                            {allDirs
                              .filter((d) => (d.files_failed ?? 0) > 0)
                              .map((d) => (
                                <button
                                  key={d.path}
                                  onClick={async () => {
                                    try { await rescanWatchDir(d.path); refreshDirStats(); if (analysisLoadedRef.current) refreshAnalysis(); }
                                    catch (e) { console.error(e); }
                                  }}
                                  className="px-3 py-1.5 rounded-md border border-sys-amber/40 bg-sys-amber/10 text-sys-amber text-[12px] hover:opacity-80 transition-opacity"
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
                          <span className="text-[15px] font-semibold text-sys-green">✓ 初始备份已完成</span>
                          <span className="text-[13px] text-t-3">{(dirStats?.total_files ?? analysis?.total_files ?? 0).toLocaleString()} 个文件受保护</span>
                        </div>
                      );
                    }

                    if (allDirs.length > 0) {
                      return (
                        <div className="flex items-center gap-2 text-[13px] text-t-3">
                          <span className="animate-spin inline-block">◐</span>
                          <span>正在启动初始扫描...</span>
                        </div>
                      );
                    }

                    return null;
                  })()}
                  {storage && (
                    <div className="text-[11px] text-t-3 leading-relaxed px-1">
                      使用 APFS clonefile 技术备份，与原文件共享磁盘空间，不产生额外占用。
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* Directory list */}
            <div>
              <div className="flex items-center justify-between mb-2">
                <h3 className="text-[13px] font-semibold text-t-1">保护目录</h3>
                <button
                  onClick={handleAddDir}
                  disabled={addingDir}
                  className="px-3.5 py-1.5 rounded-md bg-sys-blue text-white text-[12px] font-medium hover:bg-sys-blue-hover disabled:opacity-50 flex items-center gap-1.5 cursor-default"
                >
                  {addingDir ? (
                    <><span className="animate-spin text-[11px]">◐</span><span>添加中…</span></>
                  ) : (
                    <span>+ 添加目录</span>
                  )}
                </button>
              </div>

              {addSuccess && (
                <div className="mb-2 px-3 py-2 bg-sys-green-bg text-[#1a7d36] text-[12px] rounded-md leading-relaxed flex items-center gap-2">
                  <span>✓</span><span>{addSuccess}</span>
                </div>
              )}
              {addError && (
                <div className="mb-2 px-3 py-2 bg-sys-red/10 text-sys-red text-[12px] rounded-md leading-relaxed">
                  {addError}
                </div>
              )}

              <div className="space-y-2">
                {allDirs.map((dir) => (
                  <DirCard
                    key={dir.path}
                    dir={dir}
                    da={analysis?.dirs.find((d) => d.path === dir.path) || null}
                    fileCount={dirStats?.dirs.find((d) => d.path === dir.path)?.file_count ?? null}
                    expanded={expandedDir === dir.path}
                    onToggle={() => setExpandedDir(expandedDir === dir.path ? null : dir.path)}
                    onRemove={async () => {
                      if (confirm(`停止保护「${dir.name}」？\n已备份的文件不会被删除。`)) {
                        await removeWatchDir(dir.path);
                        refreshDirStats();
                        if (analysisLoadedRef.current) refreshAnalysis();
                        setAddError(null);
                      }
                    }}
                  />
                ))}
              </div>

              {allDirs.length === 0 && (
                <div className="text-center py-8 text-t-3 text-[13px]">
                  还没有保护目录，点击「+ 添加目录」开始保护
                </div>
              )}
            </div>
          </div>
        ) : tab === "ai_tools" ? (
          <AiToolsTab />
        ) : (
          <AboutTab storage={storage} fmt={fmt} />
        )}
        </div>
      </div>
    </div>
  );
}

// ─── Directory Card ───

function DirCard({
  dir, da, fileCount, expanded, onToggle, onRemove,
}: {
  dir: DirScanStatus;
  da: { total_files: number; total_bytes: number; categories: { category: string; total_bytes: number; file_count: number }[] } | null;
  fileCount: number | null;
  expanded: boolean;
  onToggle: () => void;
  onRemove: () => Promise<void>;
}) {
  return (
    <div className="border border-border rounded-md overflow-hidden bg-white">
      <button onClick={onToggle} className="w-full text-left px-4 py-3 hover:bg-bg-hover transition-colors cursor-default">
        <div className="flex items-center justify-between mb-0.5">
          <div className="flex items-center gap-2.5">
            <FolderOpen className="w-4 h-4 text-t-3 flex-shrink-0" />
            <span className="text-[13px] font-medium text-t-1">{dir.name}</span>
            {dir.status === "complete" && <span className="badge bg-sys-green-bg text-sys-green">已保护</span>}
            {dir.status === "scanning" && <span className="badge bg-sys-amber/10 text-sys-amber">扫描中 {dir.percent.toFixed(0)}%</span>}
            {dir.status === "pending" && <span className="badge bg-bg-grouped text-t-3">等待</span>}
          </div>
          {expanded ? <ChevronDown className="w-4 h-4 text-t-3" /> : <ChevronRight className="w-4 h-4 text-t-3" />}
        </div>
        <div className="text-[11px] text-t-3 ml-[26px]">{dir.path}</div>
        {da ? (
          <div className="text-[11px] text-t-2 mt-1">
            {da.total_files.toLocaleString()} 个文件 · {fmt(da.total_bytes)}
          </div>
        ) : fileCount != null ? (
          <div className="text-[11px] text-t-2 mt-1">
            {fileCount.toLocaleString()} 个文件
          </div>
        ) : null}
        {dir.status === "scanning" && (
          <div className="mt-2 w-full bg-border rounded-full h-1 overflow-hidden">
            <div className="h-full bg-sys-blue rounded-full transition-all duration-300" style={{ width: `${Math.min(dir.percent, 100)}%` }} />
          </div>
        )}
      </button>

      {expanded && (
        <div className="border-t border-border">
          {/* File type distribution */}
          {da && da.categories.length > 0 && (
            <div className="bg-bg-grouped/50 px-4 py-3">
              <div className="text-[11px] font-medium text-t-2 mb-2">文件类型分布</div>
              <div className="space-y-1.5">
                {da.categories.map((c) => {
                  const pct = da.total_bytes > 0 ? (c.total_bytes / da.total_bytes) * 100 : 0;
                  return (
                    <div key={c.category} className="flex items-center gap-2 text-[11px]">
                      <div className="w-[70px] truncate text-t-2">{c.category}</div>
                      <div className="flex-1 h-1.5 bg-border rounded-full overflow-hidden">
                        <div className="h-full rounded-full bg-sys-blue" style={{ width: `${Math.max(pct, 1)}%` }} />
                      </div>
                      <div className="w-[55px] text-right tabular-nums text-t-3">{fmt(c.total_bytes)}</div>
                      <div className="w-[45px] text-right tabular-nums text-t-3">{c.file_count} 个</div>
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
    <div className="px-4 py-3 border-t border-border-light bg-bg-grouped/20 flex items-center gap-3">
      <button
        onClick={handleClick}
        disabled={removing || dir.status === "scanning"}
        className={`flex items-center gap-1.5 text-[11px] transition-colors disabled:opacity-30 cursor-default ${
          confirm
            ? "text-sys-red font-medium"
            : "text-t-4 hover:text-sys-red"
        }`}
      >
        <Trash2 className="w-3.5 h-3.5" />
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
          className="text-[11px] text-t-4 hover:text-t-1 transition-colors cursor-default"
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
    <div className="px-4 py-4 border-t border-border-light bg-white">
      {/* Header */}
      <div className="flex items-center justify-between mb-2.5">
        <div className="text-[12px] font-medium text-t-2 flex items-center gap-1.5">
          <Info className="w-3.5 h-3.5 text-t-4" />
          过滤配置
          {config.exclude_dirs.length > 0 && (
            <span className="text-[10px] text-t-4 font-normal">
              ({config.exclude_dirs.length} 项已排除)
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {saved && <span className="text-[10px] text-sys-green flex items-center gap-0.5"><CheckCircle className="w-3 h-3" /> 已保存</span>}
          {rescanDone && <span className="text-[10px] text-sys-green flex items-center gap-0.5"><CheckCircle className="w-3 h-3" /> 扫描完成</span>}
          {isScanning ? (
            <span className="text-[10px] text-sys-blue flex items-center gap-1">
              <RefreshCw className="w-3 h-3 animate-spin" />
              {rescanning ? "重新扫描中…" : "扫描中…"}
            </span>
          ) : (
            <button
              onClick={handleRescan}
              className="flex items-center gap-1 text-[10px] text-t-3 hover:text-sys-blue transition-colors cursor-default"
              title="以当前过滤配置重新扫描此目录，更新保护统计"
            >
              <RefreshCw className="w-3 h-3" />
              重新扫描
            </button>
          )}
        </div>
      </div>

      <div className="mb-3 text-[11px] text-t-3 leading-relaxed">
        勾选后该路径及其所有子内容将被排除，不记录、不存档。规则从<b>下次变更</b>起生效。
        「重新扫描」可更新保护文件统计，但<b>不会删除</b>已有的历史存档记录。
      </div>

      {/* Tree picker for exclude dirs/files */}
      <div className="mb-3">
        <div className="text-[11px] text-t-3 mb-1">排除目录或文件</div>
        <div className="max-h-[200px] overflow-y-auto border border-border rounded-md bg-bg-grouped/30">
          <ExcludeTreeRoot
            watchDir={dirPath}
            excludeDirs={config.exclude_dirs}
            onToggle={toggleExcludePath}
          />
        </div>
      </div>

      {/* Exclude extensions */}
      <div>
        <div className="text-[11px] text-t-3 mb-1.5">排除文件类型（任意层级）</div>
        <div className="flex flex-wrap gap-1 mb-1.5">
          {config.exclude_extensions.map((e) => (
            <span
              key={e}
              className="inline-flex items-center gap-0.5 bg-bg-grouped rounded-md px-2 py-0.5 text-[11px] text-t-2"
            >
              .{e}
              <button
                onClick={() => removeExt(e)}
                className="text-t-4 hover:text-sys-red ml-0.5 leading-none cursor-default"
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
            className="flex-1 min-w-0 px-3 py-1.5 bg-bg-grouped border border-border rounded-md text-[11px] focus:outline-none focus:border-sys-blue transition-colors"
          />
          <button onClick={addExt} className="px-3 py-1.5 bg-sys-blue text-white rounded-md text-[11px] hover:bg-sys-blue-hover transition-colors cursor-default">+</button>
        </div>
      </div>
    </div>
  );
}

// ─── Recursive exclude tree ───

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
      <div className="px-3 py-2 text-[10px] text-t-4 flex items-center gap-1">
        <span className="animate-spin">◐</span> 加载中…
      </div>
    );
  }
  if (!children || children.length === 0) {
    return <div className="px-3 py-2 text-[10px] text-t-4">（无内容）</div>;
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
  const isDirectlyExcluded = excludeDirs.includes(relPath);
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
          isParentExcluded ? "opacity-40" : "hover:bg-bg-hover"
        }`}
        style={{ paddingLeft: `${8 + indent}px` }}
      >
        <input
          type="checkbox"
          checked={isDirectlyExcluded}
          disabled={isParentExcluded}
          onChange={() => onToggle(relPath)}
          className="accent-sys-blue w-3 h-3 flex-shrink-0 cursor-pointer disabled:cursor-not-allowed"
        />
        {item.is_dir ? (
          <button
            onClick={handleToggle}
            className="w-4 h-3.5 flex items-center justify-center text-[9px] text-t-4 hover:text-t-2 flex-shrink-0 cursor-default"
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
            isDirectlyExcluded ? "line-through text-t-4" : "text-t-2"
          }`}
          onClick={() => onToggle(relPath)}
          title={`${watchDir}/${relPath}`}
        >
          {item.is_dir ? `${item.name}/` : item.name}
        </span>
      </div>

      {item.is_dir && expanded && children !== null && (
        <div className="border-l border-border-light" style={{ marginLeft: `${20 + indent}px` }}>
          {children.length === 0 ? (
            <div className="px-3 py-0.5 text-[10px] text-t-4">（空）</div>
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

// ─── AI Tools Tab ───

function AiToolsTab() {
  const [tools, setTools] = useState<AiToolInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [operating, setOperating] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<{ toolId: string; ok: boolean; msg: string } | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    detectAiTools()
      .then(setTools)
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const handleToggle = async (tool: AiToolInfo) => {
    setOperating(tool.id);
    setFeedback(null);
    try {
      if (tool.hook_installed) {
        await uninstallToolHook(tool.id);
        setFeedback({ toolId: tool.id, ok: true, msg: "Hook 已移除" });
      } else {
        await installToolHook(tool.id);
        setFeedback({ toolId: tool.id, ok: true, msg: "Hook 已启用" });
      }
      refresh();
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setFeedback({ toolId: tool.id, ok: false, msg });
    } finally {
      setOperating(null);
      setTimeout(() => setFeedback(null), 4000);
    }
  };

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-[13px] font-semibold text-t-1 mb-1.5">AI 工具 Hook 管理</h3>
        <p className="text-[12px] text-t-3 leading-relaxed">
          启用 hook 后，AI 工具的每次操作都会被 rew 自动记录，可随时回溯和撤销。
          rew 会检测本机安装的 AI 工具，你可以逐个启用或关闭。
        </p>
      </div>

      {loading ? (
        <div className="flex items-center gap-2 text-[13px] text-t-2 py-4">
          <span className="inline-block animate-spin">◐</span> 正在检测 AI 工具…
        </div>
      ) : tools.length === 0 ? (
        <div className="bg-bg-grouped rounded-md px-5 py-8 text-center">
          <div className="text-[15px] text-t-2 mb-1">未检测到 AI 工具</div>
          <p className="text-[12px] text-t-3 leading-relaxed">
            目前支持 Cursor、Claude Code、Codebuddy、Workbuddy 等主流 AI 工具，在此处即可一键启用 hook。
          </p>
          <button
            onClick={refresh}
            className="mt-3 px-4 py-1.5 text-[12px] text-sys-blue hover:underline cursor-default"
          >
            重新检测
          </button>
        </div>
      ) : (
        <div className="space-y-2">
          {tools.map((tool) => {
            const isOperating = operating === tool.id;
            const fb = feedback?.toolId === tool.id ? feedback : null;
            return (
              <div
                key={tool.id}
                className="flex items-center gap-3 p-3 bg-bg-grouped rounded-md"
              >
                {getToolBrandIcon(tool.id, 28)}
                <div className="flex-1 min-w-0">
                  <div className="text-[13px] font-medium text-t-1">{tool.name}</div>
                  <div className="text-[11px] mt-0.5">
                    {tool.hook_installed ? (
                      <span className="text-sys-green flex items-center gap-1">
                        <CheckCircle className="w-3 h-3" /> Hook 已启用
                      </span>
                    ) : (
                      <span className="text-t-4">未启用</span>
                    )}
                  </div>
                  {tool.config_path && (
                    <div className="text-[10px] text-t-4 truncate mt-0.5" title={tool.config_path}>
                      {tool.config_path}
                    </div>
                  )}
                  {fb && (
                    <div
                      className={`mt-1.5 text-[11px] px-2.5 py-1 rounded-md inline-block ${
                        fb.ok
                          ? "bg-sys-green-bg text-sys-green"
                          : "bg-sys-red/10 text-sys-red"
                      }`}
                    >
                      {fb.ok ? "✓" : "✕"} {fb.msg}
                    </div>
                  )}
                </div>
                <button
                  onClick={() => handleToggle(tool)}
                  disabled={isOperating}
                  className={`text-[12px] px-2.5 py-[3px] rounded border font-medium cursor-default transition-colors disabled:opacity-50 ${
                    tool.hook_installed
                      ? "border-border bg-white text-t-2 hover:bg-bg-grouped hover:text-t-1"
                      : "border-sys-blue bg-sys-blue text-white hover:bg-sys-blue-hover"
                  }`}
                >
                  {isOperating ? (
                    <span className="flex items-center gap-1">
                      <span className="animate-spin text-[10px]">◐</span>
                      处理中…
                    </span>
                  ) : tool.hook_installed ? (
                    "关闭"
                  ) : (
                    "启用"
                  )}
                </button>
              </div>
            );
          })}

          <button
            onClick={refresh}
            className="text-[12px] text-t-3 hover:text-sys-blue transition-colors cursor-default"
          >
            重新检测
          </button>
        </div>
      )}

      <div className="space-y-2">
        <InfoNote>
          <b>启用 Hook</b> 后，AI 工具（如 Cursor、Claude Code）的文件修改会被自动记录为「AI 任务」，
          你可以在时间线中查看每次操作的详细变更，并支持一键回滚。
        </InfoNote>
        <InfoNote>
          Hook 不会影响 AI 工具的正常运行。rew 只在操作前后做快照记录，不拦截、不修改任何 AI 行为。
        </InfoNote>
      </div>
    </div>
  );
}

// ─── Shared helper ───
function InfoNote({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[12px] text-t-3 leading-relaxed p-3 bg-bg-grouped rounded-md mb-2">
      {children}
    </div>
  );
}

// ─── About Tab ───
function AboutTab({ storage, fmt }: { storage: StorageInfo | null; fmt: (b: number) => string }) {
  const { status, updateInfo, progress, error, checkForUpdates, downloadAndInstall, restart } = useUpdater();

  // 组件挂载时自动静默检查更新
  useEffect(() => {
    checkForUpdates();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="space-y-4">
      <h3 className="text-[13px] font-semibold text-t-1">rew — AI 时代的文件安全网</h3>
      <p className="text-[11px] text-t-2 leading-relaxed">
        实时监控文件，AI 工具操作时自动备份。误删或改错，一键撤销。
      </p>
      <div className="text-[11px] text-t-3 space-y-1">
        <div>版本: 0.1.0</div>
        <div>存储: APFS clonefile (CoW)</div>
        <div>作者（联系人）: kuqili</div>
        {storage && <div>备份: {storage.object_count.toLocaleString()} 对象 · {fmt(storage.apparent_bytes)}</div>}
      </div>

      {/* 有新版本时才显示更新区域 */}
      {(status === 'available' || status === 'downloading' || status === 'ready' || status === 'error') && (
        <div className="pt-3 border-t border-border space-y-2">
          <h4 className="text-[11px] font-semibold text-t-1">软件更新</h4>

          {status === 'available' && updateInfo && (
            <div className="space-y-2">
              <p className="text-[11px] text-t-2">
                发现新版本 <span className="font-semibold text-t-1">{updateInfo.version}</span>
              </p>
              {updateInfo.body && (
                <p className="text-[11px] text-t-3 leading-relaxed whitespace-pre-wrap bg-bg-grouped rounded-md p-2">
                  {updateInfo.body}
                </p>
              )}
              <button
                onClick={downloadAndInstall}
                className="px-3 py-1.5 rounded-md bg-sys-blue text-white text-[12px] font-medium hover:bg-sys-blue-hover transition-colors cursor-default"
              >
                下载并安装
              </button>
            </div>
          )}

          {status === 'downloading' && (
            <div className="space-y-1.5">
              <p className="text-[11px] text-t-3">正在下载… {progress}%</p>
              <div className="w-full h-1.5 bg-border rounded-full overflow-hidden">
                <div
                  className="h-full bg-sys-blue rounded-full transition-all duration-200"
                  style={{ width: `${progress}%` }}
                />
              </div>
            </div>
          )}

          {status === 'ready' && (
            <div className="space-y-2">
              <p className="text-[11px] text-sys-green flex items-center gap-1">
                <CheckCircle className="w-3 h-3" /> 下载完成，重启后生效
              </p>
              <button
                onClick={restart}
                className="px-3 py-1.5 rounded-md bg-sys-blue text-white text-[12px] font-medium hover:bg-sys-blue-hover transition-colors cursor-default"
              >
                立即重启
              </button>
            </div>
          )}

          {status === 'error' && (
            <p className="text-[11px] text-t-3">更新检查失败（{error}）</p>
          )}
        </div>
      )}

      <div className="pt-3 border-t border-border">
        <h4 className="text-[11px] font-semibold text-t-1 mb-2">默认不备份的文件类型</h4>
        <div className="text-[11px] text-t-3 space-y-1 leading-relaxed">
          <div>• <b>应用程序</b> — .app 包内文件</div>
          <div>• <b>安装包</b> — .dmg, .pkg, .iso, .msi, .exe</div>
          <div>• <b>开发产物</b> — node_modules, .git, target, __pycache__</div>
          <div>• <b>系统临时文件</b> — .DS_Store, Thumbs.db, .swp</div>
          <div>• <b>大部分系统运行过程中会频繁产生变更的文件（非用户文件）</b></div>
        </div>
      </div>
    </div>
  );
}
