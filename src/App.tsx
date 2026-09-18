import { useState, useMemo, useCallback, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { PlayerView } from "./components/PlayerView";
import { SearchBar } from "./components/SearchBar";
import { useVideos } from "./hooks/useVideos";
import { usePlayer } from "./hooks/usePlayer";
import { usePotPlayer } from "./hooks/usePotPlayer";
import type { Video, VideoFileStatus } from "./types";
import { filterVideos, filterByWatchState, sortVideos, selectedDirectoryLabel } from "./libraryFilter";
import type { SortField, SortDirection, WatchState } from "./libraryFilter";
import { LibraryToolbar } from "./components/LibraryToolbar";
import { ToastLayer } from "./components/Toast";
import type { ToastItem } from "./components/Toast";
import { invoke } from "@tauri-apps/api/core";

const POTPLAYER_PREF_KEY = "viewman.usePotPlayer";

interface ScanProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  warnings?: string[];
}

interface ThumbnailProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  generated: number;
  failed: number;
}

function App() {
  const { videos, progressMap, recentlyPlayed, loading, error: libraryError, clearError: clearLibraryError, scanDirectory, saveProgress, loadVideos, rescanStatus } = useVideos();
  const { currentVideo, initialPosition, openPlayer, closePlayer } = usePlayer();
  const { launch: launchInPotPlayer, error: potPlayerError, clearError: clearPotPlayerError } = usePotPlayer(saveProgress);

  const [searchQuery, setSearchQuery] = useState("");
  const [selectedDir, setSelectedDir] = useState<string | null>(null);
  const [useExternalPlayer, setUseExternalPlayer] = useState(() => localStorage.getItem(POTPLAYER_PREF_KEY) === "1");
  const [scanProgress, setScanProgress] = useState<{ processed: number; total: number } | null>(null);

  const [scanNotice, setScanNotice] = useState<string | null>(null);
  const [sortField, setSortField] = useState<SortField>("filename");
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [watchState, setWatchState] = useState<WatchState>("all");
  const [missingIds, setMissingIds] = useState<Set<string>>(() => new Set());
  const [checkProgress, setCheckProgress] = useState<{ processed: number; total: number } | null>(null);
  const checking = checkProgress !== null;
  const [thumbProgress, setThumbProgress] = useState<{ processed: number; total: number } | null>(null);
  const generating = thumbProgress !== null;
  const [toasts, setToasts] = useState<ToastItem[]>([]);

  const notify = useCallback((message: string, tone: ToastItem["tone"] = "info") => {
    const id = Date.now() + Math.random();
    setToasts(prev => [...prev, { id, message, tone }]);
  }, []);
  const dismissToast = useCallback((id: number) => {
    setToasts(prev => prev.filter(t => t.id !== id));
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<ThumbnailProgressPayload>("thumbnail-progress", (event) => {
      const { processed, total, done } = event.payload;
      setThumbProgress(done ? null : { processed, total });
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 事件监听失败不影响手动刷新 */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<ScanProgressPayload>("scan-progress", (event) => {
      const { processed, total, done, warnings } = event.payload;
      setScanProgress(done ? null : { processed, total });
      if (warnings?.length) setScanNotice(warnings.join("；"));
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(e => setScanNotice(`扫描进度监听失败：${String(e)}`));
    return () => { disposed = true; unlisten?.(); };
  }, []);

  // 自动重扫切换到新目录时，先清掉上一目录遗留的文件计数
  useEffect(() => {
    if (rescanStatus) setScanProgress(null);
  }, [rescanStatus]);

  const handleScan = useCallback(async (dir: string) => {
    setScanProgress(null);
    setScanNotice(null);
    try { await scanDirectory(dir); }
    finally { setScanProgress(null); }
  }, [scanDirectory]);

  const togglePotPlayer = useCallback(() => {
    setUseExternalPlayer((prev) => {
      const next = !prev;
      localStorage.setItem(POTPLAYER_PREF_KEY, next ? "1" : "0");
      return next;
    });
  }, []);

  const seekFor = useCallback((video: Video): number | null => {
    const pos = progressMap[video.id] ?? 0;
    return pos > 0 ? pos : null;
  }, [progressMap]);

  const handlePlayVideo = useCallback((video: Video) => {
    if (useExternalPlayer) {
      launchInPotPlayer(video, seekFor(video));
    } else {
      openPlayer(video, progressMap[video.id] ?? 0);
    }
  }, [useExternalPlayer, progressMap, openPlayer, launchInPotPlayer, seekFor]);

  const handlePlayById = useCallback((videoId: string, position: number) => {
    const video = videos.find((v) => v.id === videoId);
    if (!video) return;
    if (useExternalPlayer) {
      launchInPotPlayer(video, position > 0 ? position : null);
    } else {
      openPlayer(video, position);
    }
  }, [videos, useExternalPlayer, openPlayer, launchInPotPlayer]);

  const handleFallbackToPotPlayer = useCallback((video: Video) => {
    closePlayer();
    launchInPotPlayer(video, seekFor(video));
  }, [closePlayer, launchInPotPlayer, seekFor]);

  // 逐个校验文件可读性，只做标记，不动数据库，避免磁盘离线时误判为已删除
  const handleCheckFiles = useCallback(async () => {
    if (videos.length === 0) return;
    const unreadable = new Set<string>();
    setCheckProgress({ processed: 0, total: videos.length });
    try {
      for (let i = 0; i < videos.length; i++) {
        try {
          const result = await invoke<VideoFileStatus>("check_video_file", { videoId: videos[i].id });
          if (result.status !== "readable") unreadable.add(videos[i].id);
        } catch {
          unreadable.add(videos[i].id);
        }
        setCheckProgress({ processed: i + 1, total: videos.length });
      }
      setMissingIds(unreadable);
      setScanNotice(unreadable.size > 0
        ? `文件检查完成：${unreadable.size} 个文件当前不可读取（已标记）。`
        : "文件检查完成：所有文件均可读取。");
    } finally {
      setCheckProgress(null);
    }
  }, [videos]);

  const toggleSortDirection = useCallback(() => {
    setSortDirection(prev => (prev === "asc" ? "desc" : "asc"));
  }, []);

  const withoutThumbnailCount = useMemo(
    () => videos.filter(v => !v.thumbnail_path).length,
    [videos],
  );

  const handleGenerateThumbnails = useCallback(async () => {
    const pending = videos.filter(v => !v.thumbnail_path).map(v => v.id);
    if (pending.length === 0) {
      notify("所有视频都已有封面。");
      return;
    }
    setThumbProgress({ processed: 0, total: pending.length });
    try {
      const generated = await invoke<number>("generate_thumbnails", { videoIds: pending });
      await loadVideos();
      notify(generated > 0
        ? `已生成 ${generated} 个封面。`
        : "没有生成新封面：可能缺少 ffmpeg，或视频抽帧失败。");
    } catch (e) {
      notify(`生成封面失败：${String(e)}`, "error");
    } finally {
      setThumbProgress(null);
    }
  }, [videos, loadVideos, notify]);

  const clearMissing = useCallback(() => setMissingIds(new Set()), []);

  const filteredVideos = useMemo(() => {
    const matched = filterVideos(videos, selectedDir, searchQuery);
    const byWatchState = filterByWatchState(matched, progressMap, (v) => v.id, watchState);
    return sortVideos(byWatchState, sortField, sortDirection);
  }, [videos, selectedDir, searchQuery, progressMap, watchState, sortField, sortDirection]);

  return (
    <div className="library-shell h-screen w-screen flex text-white overflow-hidden">
      <Sidebar
        videos={videos}
        recentlyPlayed={recentlyPlayed}
        selectedDir={selectedDir}
        onSelectDir={setSelectedDir}
        onScanDirectory={handleScan}
        onPlayVideo={handlePlayById}
        loading={loading}
        scanProgress={scanProgress}
        rescanStatus={rescanStatus}
        usePotPlayer={useExternalPlayer}
        onTogglePotPlayer={togglePotPlayer}
      />
      <main className="library-main flex-1 flex flex-col gap-5 overflow-hidden">
        <SearchBar value={searchQuery} onChange={setSearchQuery} total={filteredVideos.length} />
        {(libraryError || scanNotice) && (
          <div role="alert" className="bg-amber-900/80 text-amber-100 px-3 py-2 rounded text-sm flex justify-between items-center">
            <span>{libraryError || scanNotice}</span>
            <button onClick={() => { clearLibraryError(); setScanNotice(null); }} aria-label="关闭提示" className="hover:text-white ml-2 shrink-0">✕</button>
          </div>
        )}
        {potPlayerError && (
          <div className="bg-red-900/80 text-red-200 px-3 py-2 rounded text-sm flex justify-between items-center">
            <span>{potPlayerError}</span>
            <button onClick={clearPotPlayerError} className="text-red-300 hover:text-white ml-2">✕</button>
          </div>
        )}
        <div className="flex justify-between items-center text-xs text-gray-400 shrink-0"><span className="truncate" title={selectedDir || "所有视频"}>{selectedDirectoryLabel(selectedDir)}</span><span className="ml-3 shrink-0">{searchQuery ? "搜索结果" : "本地媒体"}</span></div>
        <LibraryToolbar
          sortField={sortField}
          sortDirection={sortDirection}
          onSortFieldChange={setSortField}
          onToggleDirection={toggleSortDirection}
          watchState={watchState}
          onWatchStateChange={setWatchState}
          onCheckFiles={handleCheckFiles}
          checking={checking}
          checkProgress={checkProgress}
          missingCount={missingIds.size}
          onClearMissing={clearMissing}
          onGenerateThumbnails={handleGenerateThumbnails}
          generating={generating}
          thumbProgress={thumbProgress}
          withoutThumbnailCount={withoutThumbnailCount}
        />
        <VideoGrid videos={filteredVideos} progressMap={progressMap} missingIds={missingIds} onPlay={handlePlayVideo} onDeleted={loadVideos} />
      </main>
      {!useExternalPlayer && currentVideo && (
        <PlayerView
          key={currentVideo.id}
          video={currentVideo}
          initialPosition={initialPosition}
          onClose={closePlayer}
          onProgress={saveProgress}
          onFallback={handleFallbackToPotPlayer}
        />
      )}
      <ToastLayer toasts={toasts} onClose={dismissToast} />
    </div>
  );
}

export default App;
