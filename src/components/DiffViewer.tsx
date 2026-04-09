interface Props {
  diffText: string | null;
}

export default function DiffViewer({ diffText }: Props) {
  if (!diffText) {
    return (
      <div className="px-6 py-4 text-ink-muted text-2xs text-center">
        暂无 diff（二进制文件或未计算）
      </div>
    );
  }

  const lines = diffText.split("\n");

  return (
    <div className="overflow-x-auto max-h-[320px] overflow-y-auto">
      <table className="w-full text-2xs font-mono leading-[1.6]">
        <tbody>
          {lines.map((line, i) => {
            const cls = line.startsWith("+")
              ? "diff-add"
              : line.startsWith("-")
                ? "diff-del"
                : line.startsWith("@@")
                  ? "diff-hunk"
                  : "";

            return (
              <tr key={i} className={cls}>
                <td className="w-[1px] whitespace-nowrap px-3 text-ink-faint select-none text-right border-r border-surface-border/40">
                  {i + 1}
                </td>
                <td className="px-3 whitespace-pre">{line}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
