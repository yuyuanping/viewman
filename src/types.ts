export interface Video {
  id: string;
  path: string;
  filename: string;
  duration: number | null;
  width: number | null;
  height: number | null;
  file_size: number;
  created_at: string;
  thumbnail_path?: string | null;
}

export interface WatchProgress {
  id: string;
  video_id: string;
  position: number;
  updated_at: string;
}

export interface VideoWithProgress {
  video: Video;
  position: number | null;
}

export interface RecentlyPlayed {
  video: Video;
  position: number;
  updated_at: string;
}

export interface PotPlayerStatus {
  running: boolean;
  position: number | null;
  position_source: "live" | "remembered" | null;
  state: "running" | "unknown" | "stopped";
}

export interface VideoFileStatus {
  status: "readable" | "missing" | "unavailable" | "error";
  message: string | null;
}
