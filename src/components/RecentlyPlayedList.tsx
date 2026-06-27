import type { RecentlyPlayed } from "../types";

interface Props {
  items: RecentlyPlayed[];
  onPlay: (videoId: string, position: number) => void;
}

function formatTimeAgo(dateStr: string): string {
  const d = new Date(dateStr + "Z");
  const diff = Date.now() - d.getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "刚刚";
  if (mins < 60) return `${mins}分钟前`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}小时前`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}天前`;
  return `${Math.floor(days / 30)}月前`;
}

function formatProgress(pos: number, dur: number | null): string {
  const pct = dur && dur > 0 ? Math.min(100, Math.round((pos / dur) * 100)) : Math.round(pos);
  return `${pct}%`;
}

export function RecentlyPlayedList({ items, onPlay }: Props) {
  if (items.length === 0) return null;

  return (
    <>
      <div className="border-t border-gray-700 my-1" />
      <span className="text-xs text-gray-400 font-medium">播放记录</span>
      <div className="flex flex-col gap-0.5 overflow-y-auto max-h-48">
        {items.map(item => (
          <button
            key={item.video.id}
            onClick={() => onPlay(item.video.id, item.position)}
            className="w-full text-left px-2 py-1.5 rounded text-sm hover:bg-gray-700 flex items-center gap-2"
          >
            <span className="truncate flex-1 text-gray-300 text-xs">{item.video.filename}</span>
            <span className="text-xs text-gray-500 shrink-0">{formatProgress(item.position, item.video.duration)}</span>
            <span className="text-xs text-gray-600 shrink-0">{formatTimeAgo(item.updated_at)}</span>
          </button>
        ))}
      </div>
    </>
  );
}
