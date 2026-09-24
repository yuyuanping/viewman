import { useState, useEffect } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { DirTree } from "./DirTree";
import { RecentlyPlayedList } from "./RecentlyPlayedList";
import { countUnderDir } from "../scanRoots";
import type { RescanStatus } from "../hooks/useVideos";
import type { Image, MediaKind, Video, RecentlyPlayed as RecentlyPlayedType } from "../types";

interface SidebarProps {
  tab: MediaKind;
  onTabChange: (tab: MediaKind) => void;
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
  images: Image[];
  selectedImageDir: string | null;
  onSelectImageDir: (dir: string | null) => void;
  onScanImageDirectory: (dir: string) => Promise<void>;
  imageLoading: boolean;
  imageScanProgress: { processed: number; total: number } | null;
  imageRescanStatus: RescanStatus | null;
  roots: string[];
  removingRoot: boolean;
  onRemoveRoot: (dir: string) => Promise<void>;
}

export function Sidebar({
  tab, onTabChange,
  videos, recentlyPlayed, selectedDir, onSelectDir, onScanDirectory, onPlayVideo,
  loading, scanProgress, rescanStatus, usePotPlayer, onTogglePotPlayer,
  images, selectedImageDir, onSelectImageDir, onScanImageDirectory,
  imageLoading, imageScanProgress, imageRescanStatus,
  roots, removingRoot, onRemoveRoot,
}: SidebarProps) {
  const [potplayerOk, setPotplayerOk] = useState<boolean | null>(null);
  const [ffprobeOk, setFfprobeOk] = useState<boolean | null>(null);
  const [rootError, setRootError] = useState<string | null>(null);

  useEffect(() => {
    api.checkPotplayer().then(setPotplayerOk).catch(() => setPotplayerOk(false));
    api.checkFfprobe().then(setFfprobeOk).catch(() => setFfprobeOk(false));
  }, []);

  const [operationError, setOperationError] = useState<string | null>(null);
  const isImageTab = tab === "image";
  const activeLoading = isImageTab ? imageLoading : loading;
  const activeScanProgress = isImageTab ? imageScanProgress : scanProgress;
  const activeRescanStatus = isImageTab ? imageRescanStatus : rescanStatus;

  const handleScan = async () => {
    setOperationError(null);
    try {
      const dir = await open({ directory: true, multiple: false, title: isImageTab ? "选择图片目录" : "选择视频目录" });
      if (!dir) return;
      await (isImageTab ? onScanImageDirectory : onScanDirectory)(dir);
    } catch (e) {
      setOperationError(`无法扫描目录：${String(e)}`);
    }
  };

  const handleRemoveRoot = async (dir: string) => {
    setRootError(null);
    try {
      await onRemoveRoot(dir);
    } catch (e) {
      setRootError(`移除目录失败：${String(e)}`);
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
      <div className="flex items-center gap-3 mb-5 mt-1"><span className="w-9 h-9 rounded-xl bg-blue-600 grid place-items-center shadow-lg shadow-blue-600/20"><svg width="19" height="19" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 4v16l14-8z"/></svg></span><div><h1 className="text-lg font-semibold tracking-tight">ViewMan</h1><p className="text-[10px] text-gray-400 tracking-wider">本地媒体管理</p></div></div>
      <div className="media-tabs" role="tablist" aria-label="媒体库切换">
        <button role="tab" type="button" aria-selected={!isImageTab} className={`media-tab${!isImageTab ? " media-tab-active" : ""}`} onClick={() => onTabChange("video")}>
          视频 <span className="media-tab-count">{videos.length}</span>
        </button>
        <button role="tab" type="button" aria-selected={isImageTab} className={`media-tab${isImageTab ? " media-tab-active" : ""}`} onClick={() => onTabChange("image")}>
          图片 <span className="media-tab-count">{images.length}</span>
        </button>
      </div>
      <button
        onClick={handleScan}
        disabled={activeLoading}
        className="bg-blue-600 hover:bg-blue-500 disabled:opacity-50 py-2.5 px-4 rounded-xl text-sm font-medium shadow-lg shadow-blue-600/10"
      >
        {activeLoading
          ? (activeScanProgress ? `扫描中 ${activeScanProgress.processed}/${activeScanProgress.total}` : "扫描中...")
          : isImageTab ? "扫描图片目录" : "扫描视频目录"}
      </button>
      {activeRescanStatus && (
        <div className="rescan-progress" role="status" aria-live="polite">
          <div className="flex items-center gap-2 text-xs text-gray-300">
            <svg className="animate-spin shrink-0" width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <circle cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity="0.25" strokeWidth="4" />
              <path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" strokeWidth="4" strokeLinecap="round" />
            </svg>
            <span className="shrink-0">自动重扫 {activeRescanStatus.current}/{activeRescanStatus.total}</span>
            <span className="truncate text-gray-400" title={activeRescanStatus.dir}>{activeRescanStatus.dir}</span>
          </div>
          <div className="rescan-progress-track">
            <div
              className="rescan-progress-bar"
              style={{ width: `${Math.min(100, Math.round((((activeRescanStatus.current - 1) + (activeScanProgress && activeScanProgress.total > 0 ? Math.min(activeScanProgress.processed / activeScanProgress.total, 1) : 0)) / activeRescanStatus.total) * 100))}%` }}
            />
          </div>
          {activeScanProgress && activeScanProgress.total > 0 && (
            <span className="text-[10px] text-gray-400">新文件 {activeScanProgress.processed}/{activeScanProgress.total}</span>
          )}
        </div>
      )}
      {!isImageTab && (
        <button
          onClick={handleTogglePotPlayer}
          className={`py-1.5 px-4 rounded text-xs ${usePotPlayer ? "bg-green-700 hover:bg-green-600" : "bg-gray-700 hover:bg-gray-600"}`}
        >
          {usePotPlayer ? "PotPlayer ✓" : "外部播放器"}
        </button>
      )}
      {operationError && <span role="alert" className="text-red-300 text-xs">{operationError}</span>}
      {!isImageTab && usePotPlayer && <span className="text-gray-400 text-xs">进度同步需要 PotPlayer 标题栏显示当前时间；读取不到时会保留原进度，不自动修改播放器配置。</span>}
      {!isImageTab && potplayerOk === false && <span className="text-red-400 text-xs">✗ 未找到 PotPlayer</span>}
      {ffprobeOk === false && <span className="text-yellow-500 text-xs">✗ 未检测到 ffprobe，将无法获取时长、图片尺寸和封面</span>}
      <div className="border-t border-gray-700 my-1" />
      {roots.length > 0 && (
        <div className="flex flex-col gap-1">
          <span className="text-xs text-gray-400 font-medium">扫描目录</span>
          <ul className="flex flex-col gap-0.5 max-h-40 overflow-y-auto">
            {roots.map((dir) => {
              const name = dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || dir;
              return (
                <li key={dir} className="flex items-center gap-1 text-xs text-gray-300 group">
                  <span className="truncate flex-1" title={dir}>{name}</span>
                  <span className="text-gray-500 tabular-nums shrink-0">{countUnderDir(isImageTab ? images : videos, dir)}</span>
                  <button
                    type="button"
                    onClick={() => void handleRemoveRoot(dir)}
                    disabled={removingRoot}
                    title={`移除该目录的库内记录（磁盘文件保留），并停止自动扫描：\n${dir}`}
                    aria-label={`移除目录 ${dir}`}
                    className="shrink-0 w-5 h-5 grid place-items-center rounded text-gray-500 hover:text-red-300 hover:bg-gray-700 disabled:opacity-40"
                  >
                    ✕
                  </button>
                </li>
              );
            })}
          </ul>
          {rootError && <span role="alert" className="text-red-300 text-xs">{rootError}</span>}
        </div>
      )}
      <span className="text-xs text-gray-400 font-medium">目录</span>
      {isImageTab ? (
        <DirTree
          items={images}
          rootLabel="所有图片"
          selectedDir={selectedImageDir}
          onSelectDir={onSelectImageDir}
        />
      ) : (
        <>
          <DirTree items={videos} rootLabel="所有视频" selectedDir={selectedDir} onSelectDir={onSelectDir} />
          <RecentlyPlayedList items={recentlyPlayed} onPlay={onPlayVideo} />
        </>
      )}
    </aside>
  );
}
