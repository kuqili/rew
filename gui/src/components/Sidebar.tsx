import { useState } from "react";
import { Clock, Sparkles, FolderOpen, Settings, BarChart3 } from "lucide-react";
import { useStatus } from "../hooks/useTasks";
import { useScanProgress } from "../hooks/useScanProgress";
import { listDirContents, type DirContentItem, type DirScanStatus } from "../lib/tauri";
import type { ViewMode } from "./MainLayout";

interface Props {
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  onOpenSettings: (tab?: "dirs" | "record" | "ai_tools" | "about") => void;
  /** AI tool filter sub-items under "AI 任务历史" */
  toolFilter: string | null;
  onToolFilterChange: (tool: string | null) => void;
  activeTools: { key: string; label: string }[];
  hasUpdate?: boolean;
}

export default function Sidebar({
  selectedDir,
  onSelectDir,
  viewMode,
  onViewModeChange,
  onOpenSettings,
  toolFilter,
  onToolFilterChange,
  activeTools,
  hasUpdate = false,
}: Props) {
  const status = useStatus();
  const scanProgress = useScanProgress();
  const dirs = scanProgress?.dirs || [];

  const isRunning = status?.running ?? false;

  return (
    <aside className="w-[220px] flex-shrink-0 bg-bg-sidebar backdrop-blur-xl flex flex-col h-full select-none border-r border-border">
      {/* Status — sits at very top of column content, matching v7 */}
      <div className="px-4 py-2 flex items-center gap-2 flex-shrink-0">
        <div className={`w-[7px] h-[7px] rounded-full flex-shrink-0 ${isRunning ? "bg-sys-green" : "bg-t-4"}`} />
        <span className="text-[13px] font-medium text-t-1">
          {isRunning ? "rew 实时防护中" : "已暂停"}
        </span>
      </div>

      {/* Navigation */}
      <nav className="flex-1 overflow-y-auto overflow-x-hidden min-h-0">
        {/* WORKSPACE */}
        <SectionLabel>Workspace</SectionLabel>

        <NavItem
          icon={<Clock className="w-4 h-4" />}
          label="最近活动"
          active={viewMode === "all"}
          onClick={() => onViewModeChange("all")}
        />
        <NavItem
          icon={<svg className="w-4 h-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"><path d="M3.5 4h9M3.5 8h6M3.5 12h7.5"/></svg>}
          label="AI 任务历史"
          active={viewMode === "ai" && toolFilter === null}
          onClick={() => onViewModeChange("ai")}
        />

        {/* AI tool filter sub-items — disclosure children */}
        {viewMode === "ai" && activeTools.length >= 1 && (
          <div className="mt-0.5 mb-1">
            <SubItem
              label="全部"
              selected={toolFilter === null}
              onClick={() => onToolFilterChange(null)}
            />
            {activeTools.map(({ key, label }) => (
              <SubItem
                key={key}
                label={label}
                selected={toolFilter === key}
                onClick={() => onToolFilterChange(toolFilter === key ? null : key)}
              />
            ))}
          </div>
        )}

        <NavItem
          icon={<BarChart3 className="w-4 h-4" />}
          label="使用洞察"
          active={viewMode === "insights"}
          onClick={() => onViewModeChange("insights")}
        />

        {/* PROTECTED */}
        <SectionLabel>Protected</SectionLabel>

        {/* All directories */}
        <NavItem
          icon={<FolderOpen className="w-4 h-4" />}
          label="全部目录"
          active={selectedDir === null}
          onClick={() => onSelectDir(null)}
        />

        {/* Expandable directory trees */}
        {dirs.map((dir) => (
          <DirTreeEntry
            key={dir.path}
            dir={dir}
            selectedDir={selectedDir}
            onSelectDir={onSelectDir}
          />
        ))}

        {/* Fallback from status */}
        {(!scanProgress || (dirs.length === 0 && !scanProgress.is_scanning)) &&
          status?.watch_dirs.map((dir, i) => {
            const name = dir.split("/").pop() || dir;
            return (
              <NavItem
                key={i}
                icon={<FolderOpen className="w-4 h-4" />}
                label={name}
                active={selectedDir === dir}
                onClick={() => onSelectDir(selectedDir === dir ? null : dir)}
              />
            );
          })}
      </nav>

      {/* Footer */}
      <div className="mt-auto p-2 border-t border-border-light">
        <button
          onClick={() => onOpenSettings()}
          className="w-full flex items-center gap-2 h-[28px] px-3 mx-2 rounded text-[12px] text-t-3 hover:bg-bg-hover cursor-default transition-colors"
        >
          <div className="relative flex-shrink-0">
            <Settings className="w-[14px] h-[14px]" />
            {hasUpdate && (
              <span className="absolute -top-[3px] -right-[3px] w-[6px] h-[6px] rounded-full bg-sys-red" />
            )}
          </div>
          设置
          {hasUpdate && (
            <span className="ml-auto text-[10px] text-sys-red font-medium">有更新</span>
          )}
        </button>
      </div>
    </aside>
  );
}

// ─── Section Label ─────────────────────────────────────────────────

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[11px] font-semibold text-t-3 uppercase tracking-wider px-4 pt-3 pb-1">
      {children}
    </div>
  );
}

// ─── Nav Item ──────────────────────────────────────────────────────

function NavItem({
  icon,
  label,
  active,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-2 h-[28px] px-3 mx-2 rounded text-[13px] cursor-default transition-colors
        ${active ? "bg-sys-blue text-white" : "text-t-2 hover:bg-bg-hover"}`}
    >
      <span className={active ? "text-white" : "text-t-3"}>{icon}</span>
      <span className="truncate flex-1 text-left">{label}</span>
    </button>
  );
}

// ─── Sub Item (tool filter) ───────────────────────────────────────

function SubItem({
  label,
  selected,
  onClick,
}: {
  label: string;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center h-[26px] pl-9 pr-3 mx-2 rounded text-[13px] cursor-default transition-colors
        ${selected ? "bg-sys-blue text-white" : "text-t-2 hover:bg-bg-hover"}`}
    >
      {label}
    </button>
  );
}

// ─── Directory Tree Entry (expandable) ─────────────────────────────

function DirTreeEntry({
  dir,
  selectedDir,
  onSelectDir,
}: {
  dir: DirScanStatus;
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<DirContentItem[] | null>(null);
  const [loading, setLoading] = useState(false);

  const isSelected = selectedDir === dir.path;

  const handleToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    const willExpand = !expanded;
    if (willExpand && children === null) {
      setLoading(true);
      try {
        const items = await listDirContents(dir.path);
        setChildren(items);
      } catch {
        setChildren([]);
      } finally {
        setLoading(false);
      }
    }
    setExpanded(willExpand);
  };

  return (
    <div>
      <button
        onClick={() => onSelectDir(isSelected ? null : dir.path)}
        className={`w-full flex items-center gap-1.5 h-[28px] px-3 mx-2 rounded text-[13px] cursor-default transition-colors
          ${isSelected ? "bg-sys-blue text-white" : "text-t-2 hover:bg-bg-hover"}`}
      >
        {/* Disclosure arrow */}
        <span
          onClick={handleToggle}
          className={`text-[8px] flex-shrink-0 w-3 text-center ${
            isSelected ? "text-white" : "text-t-4"
          }`}
        >
          {loading ? (
            <span className="animate-spin inline-block">◐</span>
          ) : expanded ? (
            "▼"
          ) : (
            "▶"
          )}
        </span>

        <FolderOpen className={`w-4 h-4 flex-shrink-0 ${isSelected ? "text-white" : "text-t-3"}`} />
        <span className="truncate flex-1 text-left">{dir.name}</span>

        {dir.status === "complete" && (
          <span className={`text-[10px] flex-shrink-0 ${isSelected ? "text-white" : "text-sys-green"}`}>✓</span>
        )}
        {dir.status === "scanning" && (
          <span className={`text-[10px] tabular-nums flex-shrink-0 ${isSelected ? "text-white" : "text-t-3"}`}>
            {dir.percent?.toFixed(0)}%
          </span>
        )}
      </button>

      {/* Children */}
      {expanded && (
        <div className="pl-4">
          {children === null || loading ? (
            <div className="px-3 py-1 text-[10px] text-t-4 flex items-center gap-1">
              <span className="animate-spin">◐</span> 加载中…
            </div>
          ) : children.length === 0 ? (
            <div className="px-3 py-1 text-[10px] text-t-4">（空目录）</div>
          ) : (
            children.map((child) => (
              <SubDirRow
                key={child.full_path}
                item={child}
                depth={1}
                selectedDir={selectedDir}
                onSelectDir={onSelectDir}
              />
            ))
          )}
        </div>
      )}
    </div>
  );
}

// ─── Sub-directory/file row (recursive) ────────────────────────────

function SubDirRow({
  item,
  depth,
  selectedDir,
  onSelectDir,
}: {
  item: DirContentItem;
  depth: number;
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<DirContentItem[] | null>(null);
  const [loading, setLoading] = useState(false);

  const isSelected = selectedDir === item.full_path;

  const handleToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!item.is_dir) return;
    const willExpand = !expanded;
    if (willExpand && children === null) {
      setLoading(true);
      try {
        const items = await listDirContents(item.full_path);
        setChildren(items);
      } catch {
        setChildren([]);
      } finally {
        setLoading(false);
      }
    }
    setExpanded(willExpand);
  };

  const paddingLeft = 16 + depth * 16;

  return (
    <div>
      <button
        onClick={() => onSelectDir(isSelected ? null : item.full_path)}
        className={`w-full flex items-center gap-1.5 h-[26px] pr-3 mx-2 rounded text-[13px] cursor-default transition-colors
          ${isSelected ? "bg-sys-blue text-white" : "text-t-2 hover:bg-bg-hover"}`}
        style={{ paddingLeft: `${paddingLeft}px` }}
      >
        {/* Disclosure arrow for directories */}
        {item.is_dir ? (
          <span
            onClick={handleToggle}
            className={`text-[8px] flex-shrink-0 w-3 text-center ${
              isSelected ? "text-white" : "text-t-4"
            }`}
          >
            {loading ? (
              <span className="animate-spin inline-block">◐</span>
            ) : expanded ? (
              "▼"
            ) : (
              "▶"
            )}
          </span>
        ) : (
          <span className="w-3 flex-shrink-0" />
        )}

        <span className="truncate flex-1 text-left">
          {item.is_dir ? `${item.name}/` : item.name}
        </span>
      </button>

      {item.is_dir && expanded && children && (
        <div>
          {children.length === 0 ? (
            <div className="px-3 py-0.5 text-[10px] text-t-4" style={{ paddingLeft: `${paddingLeft + 16}px` }}>
              （空）
            </div>
          ) : (
            children.map((child) => (
              <SubDirRow
                key={child.full_path}
                item={child}
                depth={depth + 1}
                selectedDir={selectedDir}
                onSelectDir={onSelectDir}
              />
            ))
          )}
        </div>
      )}
    </div>
  );
}
