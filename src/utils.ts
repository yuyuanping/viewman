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

/** 图片尺寸标注：ffprobe 取不到宽高时留白，不显示 0×0 */
export function formatResolution(width: number | null | undefined, height: number | null | undefined): string {
  if (!width || !height) return "";
  return `${width}×${height}`;
}

/** 字节数格式化；空值/非正数的占位文案由调用方给（卡片显示 "-"，汇总显示 "0 B"） */
export function formatBytes(bytes: number | null | undefined, empty = "-"): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes <= 0) return empty;
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
  return `${parseFloat((bytes / Math.pow(1024, i)).toFixed(1))} ${units[i]}`;
}

/** 库视图里的文件大小：0/缺失显示 "0 B"（与卡片 "-" 分开，是两种使用场景） */
export function formatFileSize(bytes: number): string {
  return formatBytes(bytes, "0 B");
}

/**
 * QQ 面板的下载体积：保留它一贯的档位取整——100 以上与纯字节数不给小数，
 * 其余一位小数（"1.0 KB" 也显示成 "1.0 KB"）。别换成上面的 formatBytes：
 * 两者对 1024 的写法不同（这里 "1.0 KB"，那边 "1 KB"），换算成下载量数字会变样
 */
export function formatDlSize(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes <= 0) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let idx = 0;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx += 1;
  }
  return `${value.toFixed(value >= 100 || idx === 0 ? 0 : 1)} ${units[idx]}`;
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

/** 复制文本到剪贴板；WebView2 上走标准 Clipboard API */
export async function copyToClipboard(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}
