import { useState, useEffect } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { DirTree } from "./DirTree";
import { RecentlyPlayedList } from "./RecentlyPlayedList";
import type { RescanStatus } from "../hooks/useVideos";
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
  rescanStatus: RescanStatus | null;
  usePotPlayer: boolean;
  onTogglePotPlayer: () => void;
}

export function Sidebar({ videos, recentlyPlayed, selectedDir, onSelectDir, onScanDirectory, onPlayVideo, loading, scanProgress, rescanStatus, usePotPlayer, onTogglePotPlayer }: SidebarProps) {
  const [potplayerOk, setPotplayerOk] = useState<boolean | null>(null);
  const [ffprobeOk, setFfprobeOk] = useState<boolean | null>(null);

  useEffect(() => {
    api.checkPotplayer().then(setPotplayerOk).catch(() => setPotplayerOk(false));
    api.checkFfprobe().then(setFfprobeOk).catch(() => setFfprobeOk(false));
  }, []);

  const [operationError, setOperationError] = useState<string | null>(null);

  const handleScan = async () => {
    setOperationError(null);
    try {
      const dir = await open({ directory: true, multiple: false, title: "选择视频目录" });
      if (dir) await onScanDirectory(dir);
    } catch (e) {
      setOperationError(`无法扫描目录：${String(e)}`);
    }
  };

  const handleTogglePotPlayer = async () => {
    if (usePotPlayer) {
      onTogglePotPlayer();
      return;
    }
    try {
      const ok = await api.checkPotplayer();
      setPotplayerOk(ok);
      if (ok) onTogglePotPlayer();
    } catch (e) {
      setOperationError(`检测 PotPlayer 失败：${String(e)}`);
    }
  };

  return (
    <aside className="library-sidebar h-full text-white flex flex-col p-5 gap-3">
      <div className="flex items-center gap-3 mb-5 mt-1"><span className="w-9 h-9 rounded-xl bg-blue-600 grid place-items-center shadow-lg shadow-blue-600/20"><svg width="19" height="19" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 4v16l14-8z"/></svg></span><div><h1 className="text-lg font-semibold tracking-tight">ViewMan</h1><p className="text-[10px] text-gray-400 tracking-wider">本地视频管理</p></div></div>
      <button
        onClick={handleScan}
        disabled={loading}
        className="bg-blue-600 hover:bg-blue-500 disabled:opacity-50 py-2.5 px-4 rounded-xl text-sm font-medium shadow-lg shadow-blue-600/10"
      >
        {loading
          ? (scanProgress ? `扫描中 ${scanProgress.processed}/${scanProgress.total}` : "扫描中...")
          : "扫描目录"}
      </button>
      {rescanStatus && (
        <div className="rescan-progress" role="status" aria-live="polite">
          <div className="flex items-center gap-2 text-xs text-gray-300">
            <svg className="animate-spin shrink-0" width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <circle cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity="0.25" strokeWidth="4" />
              <path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" strokeWidth="4" strokeLinecap="round" />
            </svg>
            <span className="shrink-0">自动重扫 {rescanStatus.current}/{rescanStatus.total}</span>
            <span className="truncate text-gray-400" title={rescanStatus.dir}>{rescanStatus.dir}</span>
          </div>
          <div className="rescan-progress-track">
            <div
              className="rescan-progress-bar"
              style={{ width: `${Math.min(100, Math.round((((rescanStatus.current - 1) + (scanProgress && scanProgress.total > 0 ? Math.min(scanProgress.processed / scanProgress.total, 1) : 0)) / rescanStatus.total) * 100))}%` }}
            />
          </div>
          {scanProgress && scanProgress.total > 0 && (
            <span className="text-[10px] text-gray-400">新文件 {scanProgress.processed}/{scanProgress.total}</span>
          )}
        </div>
      )}
      <button
        onClick={handleTogglePotPlayer}
        className={`py-1.5 px-4 rounded text-xs ${usePotPlayer ? "bg-green-700 hover:bg-green-600" : "bg-gray-700 hover:bg-gray-600"}`}
      >
        {usePotPlayer ? "PotPlayer ✓" : "外部播放器"}
      </button>
      {operationError && <span role="alert" className="text-red-300 text-xs">{operationError}</span>}
      {usePotPlayer && <span className="text-gray-400 text-xs">进度同步需要 PotPlayer 标题栏显示当前时间；读取不到时会保留原进度，不自动修改播放器配置。</span>}
      {potplayerOk === false && <span className="text-red-400 text-xs">✗ 未找到 PotPlayer</span>}
      {ffprobeOk === false && <span className="text-yellow-500 text-xs">✗ 未检测到 ffprobe，将无法获取时长和进度</span>}
      <div className="border-t border-gray-700 my-1" />
      <span className="text-xs text-gray-400 font-medium">目录</span>
      <DirTree videos={videos} selectedDir={selectedDir} onSelectDir={onSelectDir} />
      <RecentlyPlayedList items={recentlyPlayed} onPlay={onPlayVideo} />
    </aside>
  );
}
