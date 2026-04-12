/**
 * DiffViewer — dark-themed unified diff with line numbers.
 */
interface Props {
  diffText: string | null;
  loading?: boolean;
}

interface DiffLine {
  type: "header" | "hunk" | "add" | "del" | "ctx";
  content: string;
  oldLineNo: number | null;
  newLineNo: number | null;
}

function parseDiff(text: string): DiffLine[] {
  const lines: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;

  for (const raw of text.split("\n")) {
    if (raw === "") continue;

    if (raw.startsWith("--- ") || raw.startsWith("+++ ")) {
      lines.push({ type: "header", content: raw, oldLineNo: null, newLineNo: null });
      continue;
    }

    if (raw.startsWith("@@")) {
      const m = raw.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) {
        oldLine = parseInt(m[1], 10);
        newLine = parseInt(m[2], 10);
      }
      lines.push({ type: "hunk", content: raw, oldLineNo: null, newLineNo: null });
      continue;
    }

    if (raw.startsWith("-")) {
      lines.push({ type: "del", content: raw.slice(1), oldLineNo: oldLine++, newLineNo: null });
      continue;
    }

    if (raw.startsWith("+")) {
      lines.push({ type: "add", content: raw.slice(1), oldLineNo: null, newLineNo: newLine++ });
      continue;
    }

    const content = raw.startsWith(" ") ? raw.slice(1) : raw;
    lines.push({ type: "ctx", content, oldLineNo: oldLine++, newLineNo: newLine++ });
  }

  return lines;
}

export default function DiffViewer({ diffText, loading }: Props) {
  if (loading) {
    return (
      <div className="px-6 py-4 text-zinc-500 text-[12px] flex items-center gap-2 bg-zinc-900">
        <span className="animate-spin inline-block">◐</span> 加载 diff...
      </div>
    );
  }

  if (!diffText) {
    return (
      <div className="px-6 py-4 text-zinc-600 text-[12px] text-center bg-zinc-900">
        二进制文件或无可用 diff
      </div>
    );
  }

  if (!diffText.includes("@@") && !diffText.startsWith("---")) {
    return (
      <div className="px-6 py-4 text-zinc-500 text-[12px] text-center bg-zinc-900">{diffText.trim()}</div>
    );
  }

  const lines = parseDiff(diffText);

  return (
    <div className="overflow-x-auto overflow-y-auto text-[11px] font-mono leading-[1.6] bg-zinc-900">
      <table className="w-full border-collapse">
        <tbody>
          {lines.map((line, i) => {
            if (line.type === "header") {
              return (
                <tr key={i} className="bg-zinc-800/50">
                  <td colSpan={3} className="px-4 py-0.5 text-zinc-500 select-none">{line.content}</td>
                </tr>
              );
            }

            if (line.type === "hunk") {
              return (
                <tr key={i} className="bg-zinc-800">
                  <td colSpan={3} className="px-4 py-1 text-blue-400 select-none text-[10px]">{line.content}</td>
                </tr>
              );
            }

            const isAdd = line.type === "add";
            const isDel = line.type === "del";

            const rowBg = isAdd
              ? "bg-emerald-950/30"
              : isDel
                ? "bg-red-950/30"
                : "";

            const sign = isAdd ? "+" : isDel ? "−" : " ";
            const signColor = isAdd
              ? "text-emerald-400"
              : isDel
                ? "text-red-400"
                : "text-zinc-700";

            const textColor = isAdd
              ? "text-emerald-300"
              : isDel
                ? "text-red-300"
                : "text-zinc-300";

            return (
              <tr key={i} className={rowBg}>
                <td className="w-[44px] text-right pr-2 pl-2 border-r border-zinc-800 text-zinc-600 select-none tabular-nums" style={{ minWidth: 44 }}>
                  {line.oldLineNo ?? ""}
                </td>
                <td className="w-[44px] text-right pr-2 pl-2 border-r border-zinc-800 text-zinc-600 select-none tabular-nums" style={{ minWidth: 44 }}>
                  {line.newLineNo ?? ""}
                </td>
                <td className="pl-3 pr-4 whitespace-pre">
                  <span className={`mr-2 ${signColor} select-none`}>{sign}</span>
                  <span className={textColor}>{line.content}</span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
