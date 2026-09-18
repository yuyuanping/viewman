import { useEffect, useRef } from "react";

interface SearchBarProps {
  value: string;
  onChange: (v: string) => void;
  total: number;
}

export function SearchBar({ value, onChange, total }: SearchBarProps) {
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
        <div><p className="library-eyebrow mb-1">YOUR PERSONAL LIBRARY</p><h2 className="library-heading">视频库</h2></div>
        <span className="text-gray-400 text-xs rounded-full border border-white/10 px-3 py-1.5 tabular-nums">{total.toLocaleString()} 个视频</span>
      </div>
      <div className="search-field">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" className="text-gray-400 shrink-0" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4.5 4.5"/></svg>
        <input ref={input} type="search" value={value} onChange={e => onChange(e.target.value)} placeholder="搜索视频文件名…" aria-label="搜索视频文件名" className="min-w-0 flex-1 bg-transparent text-sm py-3 outline-none placeholder:text-gray-500" />
        {value ? <button onClick={() => { onChange(""); input.current?.focus(); }} className="text-xs text-gray-400 hover:text-white" aria-label="清空搜索">清空</button> : <kbd className="hidden sm:block text-[10px] text-gray-500 border border-white/10 rounded px-1.5 py-0.5 shrink-0">Ctrl K</kbd>}
      </div>
    </header>
  );
}
