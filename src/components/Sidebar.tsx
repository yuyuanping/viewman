import { useState, useEffect } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { DirTree } from "./DirTree";
import { RecentlyPlayedList } from "./RecentlyPlayedList";
import type { Video, RecentlyPlayed as RecentlyPlayedType } from "../types";

interface SidebarProps {
  videos: Video[];
  recentlyPlayed: RecentlyPlayedType[];
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
  onScanDirectory: (dir: string) => Promise<void>;
  onPlayVideo: (videoId: string, position: number) => void;
  loading: boolean;
  scanProgress: { processed: number; total: number } | null;
  usePotPlayer: boolean;
  onTogglePotPlayer: () => void;
}

export function Sidebar({ videos, recentlyPlayed, selectedDir, onSelectDir, onScanDirectory, onPlayVideo, loading, scanProgress, usePotPlayer, onTogglePotPlayer }: SidebarProps) {
  const [potplayerOk, setPotplayerOk] = useState<boolean | null>(null);
  const [ffprobeOk, setFfprobeOk] = useState<boolean | null>(null);

  useEffect(() => {
    invoke<boolean>("check_potplayer").then(setPotplayerOk).catch(() => setPotplayerOk(false));
    invoke<boolean>("check_ffprobe").then(setFfprobeOk).catch(() => setFfprobeOk(false));
  }, []);

  // 进度回写依赖 PotPlayer 标题栏显示时间，切到外部播放器模式时自动开启一次（幂等）
  useEffect(() => {
    if (!usePotPlayer) return;
    invoke<boolean>("check_potplayer").then((ok) => {
      if (ok) invoke("enable_potplayer_titlebar").catch(() => {});
    }).catch(() => {});
  }, [usePotPlayer]);

  const handleScan = async () => {
    const dir = await open({ directory: true, multiple: false, title: "选择视频目录" });
    if (dir) {
      await onScanDirectory(dir);
    }
  };

  const handleTogglePotPlayer = async () => {
    if (usePotPlayer) {
      onTogglePotPlayer();
      return;
    }
    const ok = await invoke<boolean>("check_potplayer");
    setPotplayerOk(ok);
    if (ok) {
      onTogglePotPlayer();
    }
  };

  return (
    <aside className="w-60 h-full bg-gray-900 text-white flex flex-col p-4 gap-3">
      <h1 className="text-lg font-bold mb-1">ViewMan</h1>
      <button
        onClick={handleScan}
        disabled={loading}
        className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 py-2 px-4 rounded text-sm"
      >
        {loading
          ? (scanProgress ? `扫描中 ${scanProgress.processed}/${scanProgress.total}` : "扫描中...")
          : "扫描目录"}
      </button>
      <button
        onClick={handleTogglePotPlayer}
        className={`py-1.5 px-4 rounded text-xs ${usePotPlayer ? "bg-green-700 hover:bg-green-600" : "bg-gray-700 hover:bg-gray-600"}`}
      >
        {usePotPlayer ? "PotPlayer ✓" : "外部播放器"}
      </button>
      {potplayerOk === false && <span className="text-red-400 text-xs">✗ 未找到 PotPlayer</span>}
      {ffprobeOk === false && <span className="text-yellow-500 text-xs">✗ 未检测到 ffprobe，将无法获取时长和进度</span>}
      <div className="border-t border-gray-700 my-1" />
      <span className="text-xs text-gray-400 font-medium">目录</span>
      <DirTree videos={videos} selectedDir={selectedDir} onSelectDir={onSelectDir} />
      <RecentlyPlayedList items={recentlyPlayed} onPlay={onPlayVideo} />
    </aside>
  );
}
