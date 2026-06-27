interface SearchBarProps {
  value: string;
  onChange: (value: string) => void;
  total: number;
}

export function SearchBar({ value, onChange, total }: SearchBarProps) {
  return (
    <div className="flex gap-3 items-center">
      <input
        value={value}
        onChange={e => onChange(e.target.value)}
        placeholder="搜索视频..."
        className="flex-1 bg-gray-800 text-white border border-gray-700 rounded px-3 py-2 outline-none focus:border-blue-500"
      />
      <span className="text-sm text-gray-400">{total} 个视频</span>
    </div>
  );
}
