import type { Video } from "../types";

interface PlayerViewProps {
  video: Video;
  initialPosition: number;
  onClose: () => void;
  onProgress: (videoId: string, position: number) => Promise<void>;
}

export function PlayerView({ video, initialPosition, onClose, onProgress }: PlayerViewProps) {
  return (
    <div className="fixed inset-0 bg-black z-50 flex flex-col items-center justify-center">
      <button
        onClick={onClose}
        className="absolute top-4 right-4 bg-red-600 text-white px-4 py-2 rounded"
      >
        关闭
      </button>
      <p className="text-white text-lg">{video.filename}</p>
    </div>
  );
}
