/** AI tool display metadata — extensible registry for current and future tools. */
export interface ToolMeta {
  label: string;
  badgeClass: string;
}

export const TOOL_REGISTRY: Record<string, ToolMeta> = {
  cursor:        { label: "Cursor",      badgeClass: "bg-[#7c3aed]/10 text-[#7c3aed]" },
  "claude-code": { label: "Claude Code", badgeClass: "bg-[#e67e22]/10 text-[#e67e22]" },
  windsurf:      { label: "Windsurf",    badgeClass: "bg-[#06b6d4]/10 text-[#06b6d4]" },
  copilot:       { label: "Copilot",     badgeClass: "bg-[#1f883d]/10 text-[#1f883d]" },
  aider:         { label: "Aider",       badgeClass: "bg-[#8b5cf6]/10 text-[#8b5cf6]" },
  "ai-tool":     { label: "AI",          badgeClass: "bg-st-blue-light text-st-blue" },
};

export function getToolMeta(tool: string | null): ToolMeta | null {
  if (!tool) return null;
  if (TOOL_REGISTRY[tool]) return TOOL_REGISTRY[tool];
  const normalized = tool.toLowerCase().replace(/_/g, "-");
  if (TOOL_REGISTRY[normalized]) return TOOL_REGISTRY[normalized];
  if (tool === "文件监听" || tool === "手动存档") return null;
  return { label: tool, badgeClass: "bg-st-blue-light text-st-blue" };
}
