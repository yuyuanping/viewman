export function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) {
    return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  }
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "--:--";
  return formatTime(seconds);
}

export function formatFileSize(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(sizes.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${parseFloat((bytes / Math.pow(1024, i)).toFixed(1))} ${sizes[i]}`;
}

// 系统装了 HEVC 视频扩展时 WebView2 能原生解码，无需转码兜底
export const hevcNativeSupported: boolean = (() => {
  try {
    return document.createElement("video")
      .canPlayType('video/mp4; codecs="hvc1.1.6.L93.B0"') !== "";
  } catch {
    return false;
  }
})();
