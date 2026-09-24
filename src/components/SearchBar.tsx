import { useEffect, useRef } from "react";
import { formatDuration, formatFileSize } from "../utils";

interface SearchBarProps {
  value: string;
  onChange: (v: string) => void;
  total: number;
  title: string;
  /** 媒体名称，用于"搜索X文件名" */
  unit: string;
  /** 计数量词：视频用"个"，图片用"张" */
  measure?: string;
  /** 当前列表的总时长（秒），视频库显示；图片库不传 */
  totalDuration?: number | null;
  /** 当前列表的总大小（字节） */
  totalSize?: number;
}

export function SearchBar({ value, onChange, total, title, unit, measure = "个", totalDuration = null, totalSize = 0 }: SearchBarProps) {
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        input.current?.focus();
      }
    };
    window.addEventListener("keydown", focusSearch);
    return () => window.removeEventListener("keydown", focusSearch);
  }, []);

  return (
    <header className="flex flex-col gap-5 shrink-0">
      <div className="flex items-end justify-between gap-3">
        <div><h2 className="library-heading">{title}</h2></div>
        <span className="text-gray-400 text-xs rounded-full border border-white/10 px-3 py-1.5 tabular-nums" title={totalDuration ? "当前列表 · 总时长 · 总大小" : "当前列表 · 总大小"}>
          {total.toLocaleString()} {measure}{unit}
          {totalDuration ? <> · {formatDuration(totalDuration)}</> : null}
          {totalSize > 0 ? <> · {formatFileSize(totalSize)}</> : null}
        </span>
      </div>
      <div className="search-field">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" className="text-gray-400 shrink-0" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>
        <input ref={input} type="search" value={value} onChange={e => onChange(e.target.value)} placeholder={`搜索${unit}文件名…`} aria-label={`搜索${unit}文件名`} className="min-w-0 flex-1 bg-transparent text-sm py-3 outline-none placeholder:text-gray-500" />
        {value ? <button onClick={() => { onChange(""); input.current?.focus(); }} className="text-xs text-gray-400 hover:text-white" aria-label="清空搜索">清空</button> : <kbd className="hidden sm:block text-[10px] text-gray-500 border border-white/10 rounded px-1.5 py-0.5 shrink-0">Ctrl K</kbd>}
      </div>
    </header>
  );
}
