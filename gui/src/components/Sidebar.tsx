import { useState, useRef, useCallback } from "react";
import { useStatus } from "../hooks/useTasks";
import { useScanProgress } from "../hooks/useScanProgress";
import { listDirContents, type DirContentItem, type DirScanStatus } from "../lib/tauri";

interface Props {
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
  onOpenSettings: () => void;
  width: number;
  onWidthChange: (w: number) => void;
}

export default function Sidebar({
  selectedDir,
  onSelectDir,
  onOpenSettings,
  width,
  onWidthChange,
}: Props) {
  const status = useStatus();
  const scanProgress = useScanProgress();
  const [search, setSearch] = useState("");

  const dirs = scanProgress?.dirs || [];

  // Drag-to-resize handle
  const dragging = useRef(false);
  const onResizeMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      dragging.current = true;
      const startX = e.clientX;
      const startW = width;
      const onMove = (me: MouseEvent) => {
        if (!dragging.current) return;
        const newW = Math.max(140, Math.min(startW + me.clientX - startX, 380));
        onWidthChange(newW);
      };
      const onUp = () => {
        dragging.current = false;
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [width, onWidthChange]
  );

  const normalizedSearch = search.trim().toLowerCase();

  return (
    <aside
      className="relative bg-surface-sidebar flex flex-col h-full shadow-sidebar select-none flex-shrink-0"
      style={{ width }}
    >
      {/* Drag region / title */}
      <div data-tauri-drag-region className="h-[38px] flex items-end px-4 pb-1">
        <span className="text-2xs font-semibold text-ink-muted uppercase tracking-wider">
          WORKSPACE
        </span>
      </div>

      {/* Navigation */}
      <nav className="mt-1 flex-1 overflow-y-auto overflow-x-hidden min-h-0">
        <div className="nav-item active">
          <span className="nav-icon">🕐</span>
          <span>存档</span>
        </div>

        <div className="mt-6 px-4 mb-2">
          <span className="text-2xs font-semibold text-ink-muted uppercase tracking-wider">
            保护状态
          </span>
        </div>

        <div className="nav-item">
          <span className="nav-icon">🛡️</span>
          <span>{status?.running ? "保护中" : "已暂停"}</span>
          {status?.running && (
            <span className="ml-auto w-2 h-2 rounded-full bg-status-green" />
          )}
        </div>

        {/* Protected directories */}
        <div className="mt-5">
          <div className="px-3 mb-2">
            <span className="text-2xs font-semibold text-ink-muted uppercase tracking-wider block mb-1.5">
              保护目录
            </span>
            {/* Search box */}
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="搜索目录或文件…"
              className="w-full px-2 py-1 rounded text-[11px] bg-surface-hover border border-surface-border/60 text-ink-secondary placeholder-ink-faint focus:outline-none focus:border-st-blue/60 focus:bg-white transition-colors"
            />
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

          {/* All directories option */}
          {!normalizedSearch && (
            <button
              onClick={() => onSelectDir(null)}
              className={`w-full flex items-center gap-2 px-4 py-1.5 text-2xs cursor-pointer transition-colors ${
                selectedDir === null
                  ? "bg-st-blue/10 text-st-blue font-medium border-l-2 border-st-blue"
                  : "text-ink-secondary hover:bg-surface-hover border-l-2 border-transparent"
              }`}
            >
              <span className="w-3 flex-shrink-0 text-center text-[10px]">◉</span>
              <span className="truncate flex-1 text-left">全部目录</span>
            </button>
          )}

          {/* Directory list with expandable sub-dirs and files */}
          <div className="space-y-0">
            {dirs.map((dir) => (
              <DirEntry
                key={dir.path}
                dir={dir}
                selectedDir={selectedDir}
                onSelectDir={onSelectDir}
                searchQuery={normalizedSearch}
              />
            ))}

            {/* Fallback: show from status if scan progress not loaded yet */}
            {!scanProgress &&
              status?.watch_dirs.map((dir, i) => {
                const name = dir.split("/").pop() || dir;
                const isSelected = selectedDir === dir || selectedDir?.startsWith(dir + "/");
                return (
                  <button
                    key={i}
                    onClick={() => onSelectDir(dir)}
                    className={`w-full flex items-center gap-2 px-4 py-1 text-2xs cursor-pointer transition-colors ${
                      isSelected
                        ? "bg-st-blue/10 text-st-blue border-l-2 border-st-blue"
                        : "text-ink-secondary hover:bg-surface-hover border-l-2 border-transparent"
                    }`}
                    title={dir}
                  >
                    <span className="opacity-40">○</span>
                    <span className="truncate text-left">{name}</span>
                  </button>
                );
              })}
          </div>
        </div>
      </nav>

      {/* Bottom: manage button */}
      <div className="p-3 border-t border-surface-border">
        <button
          onClick={onOpenSettings}
          className="w-full flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-md text-2xs text-ink-secondary hover:bg-surface-hover hover:text-ink transition-colors"
        >
          <span>⚙</span>
          <span>管理保护目录</span>
        </button>
      </div>

      {/* Right-edge resize handle */}
      <div
        onMouseDown={onResizeMouseDown}
        className="absolute top-0 right-0 w-[4px] h-full cursor-col-resize hover:bg-st-blue/30 transition-colors z-10"
      />
    </aside>
  );
}

function DirEntry({
  dir,
  selectedDir,
  onSelectDir,
  searchQuery,
}: {
  dir: DirScanStatus;
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
  searchQuery: string;
}) {
  const isSelected = selectedDir === dir.path;
  const hasChildSelected = selectedDir !== null && selectedDir.startsWith(dir.path + "/");

  // When searching, filter the name at root level too
  if (searchQuery && !dir.name.toLowerCase().includes(searchQuery) && !hasChildSelected) {
    // Still show if a child is selected and search matches nothing at root, keep root visible
    // Actually if search doesn't match root name, we still show it so children can be searched
  }

  return (
    <div>
      <ContentRow
        fullPath={dir.path}
        name={dir.name}
        isDir={true}
        depth={0}
        selectedDir={selectedDir}
        onSelectDir={onSelectDir}
        searchQuery={searchQuery}
        statusIcon={
          dir.status === "complete" ? (
            <span className={isSelected || hasChildSelected ? "text-st-blue" : "text-status-green"}>
              ✓
            </span>
          ) : dir.status === "scanning" ? (
            <span className="inline-block animate-spin">◐</span>
          ) : (
            <span className="opacity-30">○</span>
          )
        }
        rightLabel={
          dir.status === "scanning" ? (
            <span className="text-[10px] text-ink-faint tabular-nums flex-shrink-0">
              {dir.percent?.toFixed(0)}%
            </span>
          ) : null
        }
        title={dir.path}
      />
    </div>
  );
}

/** Recursively expandable directory/file row. */
function ContentRow({
  fullPath,
  name,
  isDir,
  depth,
  selectedDir,
  onSelectDir,
  searchQuery,
  statusIcon,
  rightLabel,
  title,
}: {
  fullPath: string;
  name: string;
  isDir: boolean;
  depth: number;
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
  searchQuery: string;
  statusIcon?: React.ReactNode;
  rightLabel?: React.ReactNode;
  title?: string;
}) {
  // Auto-expand root dirs when searching
  const [expandedByUser, setExpandedByUser] = useState<boolean | null>(null);
  const [children, setChildren] = useState<DirContentItem[] | null>(null);
  const [loading, setLoading] = useState(false);

  const isSearching = searchQuery.length > 0;
  // When searching, auto-expand roots (depth=0)
  const expanded = isSearching && depth === 0 ? true : (expandedByUser ?? false);

  const isSelected = selectedDir === fullPath;
  const hasChildSelected =
    selectedDir !== null && selectedDir.startsWith(fullPath + "/");

  const handleToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!isDir) return;
    const willExpand = !(expandedByUser ?? false);
    if (willExpand && children === null) {
      setLoading(true);
      try {
        const items = await listDirContents(fullPath);
        setChildren(items);
      } catch {
        setChildren([]);
      } finally {
        setLoading(false);
      }
    }
    // Auto-load when searching + expanding root for first time
    if (isSearching && depth === 0 && children === null && !loading) {
      setLoading(true);
      listDirContents(fullPath)
        .then((items) => setChildren(items))
        .catch(() => setChildren([]))
        .finally(() => setLoading(false));
    }
    setExpandedByUser(willExpand);
  };

  // Load children automatically when search starts (root dirs only)
  if (isSearching && depth === 0 && children === null && !loading) {
    setLoading(true);
    listDirContents(fullPath)
      .then((items) => setChildren(items))
      .catch(() => setChildren([]))
      .finally(() => setLoading(false));
  }

  // Filter children by search query
  const visibleChildren = children
    ? isSearching
      ? children.filter((c) => c.name.toLowerCase().includes(searchQuery))
      : children
    : null;

  // Whether this row itself matches the search
  const matchesSearch = !isSearching || name.toLowerCase().includes(searchQuery);

  const indent = depth * 10;

  const rowClasses = `group flex items-center gap-1 py-1.5 text-2xs cursor-pointer transition-colors ${
    isSelected || hasChildSelected
      ? "bg-st-blue/10 text-st-blue font-medium border-l-2 border-st-blue"
      : "text-ink-secondary hover:bg-surface-hover border-l-2 border-transparent"
  }`;

  // File extension icon
  const fileIcon = (() => {
    if (isDir) return null;
    const ext = name.split(".").pop()?.toLowerCase();
    if (["ts", "tsx", "js", "jsx"].includes(ext ?? "")) return "📄";
    if (["css", "scss", "less"].includes(ext ?? "")) return "🎨";
    if (["json", "toml", "yaml", "yml"].includes(ext ?? "")) return "⚙";
    if (["md", "txt", "rst"].includes(ext ?? "")) return "📝";
    if (["rs", "py", "go", "java", "cpp", "c", "h"].includes(ext ?? "")) return "📄";
    if (["png", "jpg", "jpeg", "gif", "svg", "webp"].includes(ext ?? "")) return "🖼";
    return "📄";
  })();

  return (
    <div>
      {/* Only show this row if it matches search (or is a parent that has matching children) */}
      {(matchesSearch || hasChildSelected || (isDir && isSearching)) && (
        <div
          className={rowClasses}
          style={{ paddingLeft: `${12 + indent}px`, paddingRight: "8px" }}
          title={title ?? fullPath}
          onClick={() => onSelectDir(fullPath)}
        >
          {/* Expand toggle for dirs, file icon for files */}
          {isDir ? (
            <button
              onClick={handleToggle}
              className="w-4 h-4 flex items-center justify-center flex-shrink-0 text-[10px] text-ink-faint hover:text-ink-secondary rounded transition-colors"
            >
              {loading ? (
                <span className="animate-spin">◐</span>
              ) : expanded ? (
                "▾"
              ) : (
                "▸"
              )}
            </button>
          ) : (
            <span className="w-4 h-4 flex items-center justify-center flex-shrink-0 text-[10px] opacity-60">
              {fileIcon}
            </span>
          )}

          {/* Status icon (only for root dirs) */}
          {statusIcon && (
            <span className="w-3 flex-shrink-0 text-center text-[10px]">{statusIcon}</span>
          )}

          <span className="truncate flex-1 min-w-0">{name}</span>
          {rightLabel}
        </div>
      )}

      {/* Recursive children */}
      {isDir && expanded && (
        <div
          className="border-l border-surface-border/40"
          style={{ marginLeft: `${20 + indent}px` }}
        >
          {children === null || loading ? (
            <div className="px-3 py-1 text-[10px] text-ink-faint flex items-center gap-1">
              <span className="animate-spin">◐</span>
              <span>加载中…</span>
            </div>
          ) : (visibleChildren ?? []).length === 0 ? (
            <div className="px-3 py-1 text-[10px] text-ink-faint">
              {isSearching ? "无匹配项" : "（空目录）"}
            </div>
          ) : (
            (visibleChildren ?? []).map((child) => (
              <ContentRow
                key={child.full_path}
                fullPath={child.full_path}
                name={child.is_dir ? `${child.name}/` : child.name}
                isDir={child.is_dir}
                depth={depth + 1}
                selectedDir={selectedDir}
                onSelectDir={onSelectDir}
                searchQuery={searchQuery}
              />
            ))
          )}
        </div>
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
