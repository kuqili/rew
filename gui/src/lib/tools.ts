/** AI tool display metadata — extensible registry for current and future tools. */
export interface ToolMeta {
  label: string;
  color: string;
  badgeClass: string;
}

export const TOOL_REGISTRY: Record<string, ToolMeta> = {
  cursor: {
    label: "Cursor",
    color: "#6366f1",
    badgeClass: "bg-tool-cursor-bg text-tool-cursor",
  },
  "claude-code": {
    label: "Claude Code",
    color: "#da7b3a",
    badgeClass: "bg-tool-claude-bg text-tool-claude",
  },
  windsurf: {
    label: "Windsurf",
    color: "#06b6d4",
    badgeClass: "bg-cyan-50 text-cyan-600",
  },
  copilot: {
    label: "Copilot",
    color: "#1f883d",
    badgeClass: "bg-sys-green-bg text-sys-green",
  },
  aider: {
    label: "Aider",
    color: "#6366f1",
    badgeClass: "bg-tool-cursor-bg text-tool-cursor",
  },
};

export function getToolMeta(tool: string | null): ToolMeta | null {
  if (!tool) return null;
  if (TOOL_REGISTRY[tool]) return TOOL_REGISTRY[tool];
  const normalized = tool.toLowerCase().replace(/_/g, "-");
  if (TOOL_REGISTRY[normalized]) return TOOL_REGISTRY[normalized];
  if (tool === "文件监听" || tool === "手动存档") return null;
  return { label: tool, color: "#007aff", badgeClass: "bg-sys-blue/10 text-sys-blue" };
}
