import type { Video } from "../types";

interface VideoCardProps {
  video: Video;
  progress: number | null;
  onPlay: (video: Video) => void;
}

function formatDuration(seconds: number | null): string {
  if (seconds === null || seconds === undefined) return "--:--";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

export function VideoCard({ video, progress, onPlay }: VideoCardProps) {
  const progressPct = progress !== null && video.duration && video.duration > 0
    ? Math.min(100, (progress / video.duration) * 100)
    : 0;

  return (
    <div
      onClick={() => onPlay(video)}
      className="bg-gray-800 rounded-lg overflow-hidden cursor-pointer hover:ring-2 hover:ring-blue-500 transition-all group"
    >
      <div className="aspect-video bg-gray-700 flex items-center justify-center text-gray-500">
        <svg className="w-12 h-12" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z" />
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
        </svg>
      </div>
      <div className="p-3">
        <p className="text-sm truncate" title={video.filename}>{video.filename}</p>
        <div className="flex justify-between text-xs text-gray-400 mt-1">
          <span>{formatDuration(video.duration)}</span>
          <span>{formatFileSize(video.file_size)}</span>
        </div>
        {progressPct > 0 && (
          <div className="w-full bg-gray-600 h-1 rounded mt-2">
            <div
              className="bg-blue-500 h-1 rounded transition-all"
              style={{ width: `${progressPct}%` }}
            />
          </div>
        )}
      </div>
    </div>
  );
}
