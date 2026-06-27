interface SearchBarProps {
  value: string;
  onChange: (v: string) => void;
  total: number;
}

export function SearchBar({ value, onChange, total }: SearchBarProps) {
  return (
    <div className="flex items-center gap-3">
      <input
        type="text"
        value={value}
        onChange={e => onChange(e.target.value)}
        placeholder="搜索视频文件名..."
        className="flex-1 bg-gray-800 text-white px-4 py-2 rounded outline-none focus:ring-2 focus:ring-blue-500"
      />
      <span className="text-gray-400 text-sm">{total} 个视频</span>
    </div>
  );
}
