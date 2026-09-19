import { useState, useMemo, useCallback, useEffect } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { PlayerView } from "./components/PlayerView";
import { SearchBar } from "./components/SearchBar";
import { useVideos } from "./hooks/useVideos";
import { usePlayer } from "./hooks/usePlayer";
import { usePotPlayer } from "./hooks/usePotPlayer";
import { useToasts } from "./hooks/useToasts";
import { useScanProgress } from "./hooks/useScanProgress";
import { useThumbnailGeneration } from "./hooks/useThumbnailGeneration";
import { useHevcConversion } from "./hooks/useHevcConversion";
import { useDuplicates } from "./hooks/useDuplicates";
import { useFileCheck } from "./hooks/useFileCheck";
import { api } from "./api";
import type { Video } from "./types";
import { filterVideos, filterByWatchState, sortVideos, selectedDirectoryLabel } from "./libraryFilter";
import type { SortField, SortDirection, WatchState } from "./libraryFilter";
import { LibraryToolbar } from "./components/LibraryToolbar";
import { ToastLayer } from "./components/Toast";

const POTPLAYER_PREF_KEY = "viewman.usePotPlayer";

function App() {
  const { videos, progressMap, recentlyPlayed, loading, error: libraryError, clearError: clearLibraryError, scanDirectory, saveProgress, loadVideos, rescanStatus } = useVideos();
  const { currentVideo, initialPosition, openPlayer, closePlayer } = usePlayer();
  const { launch: launchInPotPlayer, error: potPlayerError, clearError: clearPotPlayerError } = usePotPlayer(saveProgress);

  const [searchQuery, setSearchQuery] = useState("");
  const [selectedDir, setSelectedDir] = useState<string | null>(null);
  const [useExternalPlayer, setUseExternalPlayer] = useState(() => localStorage.getItem(POTPLAYER_PREF_KEY) === "1");
  const [notice, setNotice] = useState<string | null>(null);
  const clearNotice = useCallback(() => setNotice(null), []);
  const [sortField, setSortField] = useState<SortField>("filename");
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [watchState, setWatchState] = useState<WatchState>("all");
  // 多选批量删除模式
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [deletingSelected, setDeletingSelected] = useState(false);
  // 播放器打开那一刻的列表快照：播放期间固定不变，不随库/排序/进度刷新而变
  const [playlist, setPlaylist] = useState<Video[] | null>(null);

  const { toasts, notify, dismiss } = useToasts();
  const { scanProgress, resetScanProgress } = useScanProgress(rescanStatus, setNotice);
  const { thumbProgress, generating, generateAll: handleGenerateThumbnails } = useThumbnailGeneration(videos, loadVideos, notify);
  const {
    hevcCount, hevcDetected, detecting: detectingHevc, detect: handleDetectHevc,
    converting: convertingHevc, convert: handleConvertHevc, hevcProgress, clearDetected: clearHevc,
  } = useHevcConversion(loadVideos, notify);
  const {
    duplicateGroupCount, duplicateExtrasCount, duplicateIds, duplicatesDetected,
    detecting: detectingDuplicates, detect: handleDetectDuplicates,
    deleting: deletingDuplicates, deleteExtras: handleDeleteDuplicates, clear: clearDuplicates,
  } = useDuplicates(loadVideos, notify);
  const {
    missingIds, clearMissing,
    fakeIds, clearFake, convertFakes, converting,
    shortIds, convertShorts, convertingShorts, shortsDetected, detectShorts, detecting, clearShorts,
    checkProgress, checking, checkFiles: handleCheckFiles,
  } = useFileCheck(videos, setNotice, loadVideos);

  const handleScan = useCallback(async (dir: string) => {
    resetScanProgress();
    setNotice(null);
    try { await scanDirectory(dir); }
    finally { resetScanProgress(); }
  }, [scanDirectory, resetScanProgress]);

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

  const handleClosePlayer = useCallback(() => {
    setPlaylist(null);
    closePlayer();
  }, [closePlayer]);

  const handleFallbackToPotPlayer = useCallback((video: Video) => {
    handleClosePlayer();
    launchInPotPlayer(video, seekFor(video));
  }, [handleClosePlayer, launchInPotPlayer, seekFor]);

  const toggleSortDirection = useCallback(() => {
    setSortDirection(prev => (prev === "asc" ? "desc" : "asc"));
  }, []);

  const withoutThumbnailCount = useMemo(
    () => videos.filter(v => !v.thumbnail_path).length,
    [videos],
  );

  const filteredVideos = useMemo(() => {
    const matched = filterVideos(videos, selectedDir, searchQuery);
    const byWatchState = filterByWatchState(matched, progressMap, (v) => v.id, watchState);
    return sortVideos(byWatchState, sortField, sortDirection);
  }, [videos, selectedDir, searchQuery, progressMap, watchState, sortField, sortDirection]);

  // 筛选条件一变，之前勾选但已不在视图里的项不再可见，直接清空选择避免"隐形删除"
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedDir, searchQuery, watchState]);

  const toggleSelect = useCallback((video: Video) => {
    setSelectedIds(prev => {
      const next = new Set(prev);
      if (next.has(video.id)) next.delete(video.id); else next.add(video.id);
      return next;
    });
  }, []);

  const allSelected = filteredVideos.length > 0 && filteredVideos.every(v => selectedIds.has(v.id));
  const toggleSelectAll = useCallback(() => {
    setSelectedIds(allSelected ? new Set() : new Set(filteredVideos.map(v => v.id)));
  }, [allSelected, filteredVideos]);

  const exitSelectMode = useCallback(() => {
    setSelectMode(false);
    setSelectedIds(new Set());
  }, []);

  const handleDeleteSelected = useCallback(async () => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    if (!confirm(`确定将选中的 ${ids.length} 个视频移入回收站？`)) return;
    setDeletingSelected(true);
    let ok = 0;
    let failed = 0;
    for (const id of ids) {
      try {
        await api.deleteVideo(id);
        ok += 1;
      } catch {
        failed += 1;
      }
    }
    setSelectedIds(new Set());
    await loadVideos();
    notify(failed > 0 ? `已删除 ${ok} 个，${failed} 个失败（可能被占用）` : `已将 ${ok} 个视频移入回收站。`, failed > 0 ? "error" : "info");
    setDeletingSelected(false);
  }, [selectedIds, loadVideos, notify]);

  // 从库/侧栏打开视频：以当前列表为快照固定下来；当前视频不在其中则补到最前
  const openFromLibrary = useCallback((video: Video, position?: number) => {
    const snap = filteredVideos.some((v) => v.id === video.id)
      ? filteredVideos
      : [video, ...filteredVideos];
    setPlaylist(snap);
    openPlayer(video, position ?? progressMap[video.id] ?? 0);
  }, [filteredVideos, openPlayer, progressMap]);

  const handlePlayVideo = useCallback((video: Video) => {
    if (useExternalPlayer) {
      launchInPotPlayer(video, seekFor(video));
    } else {
      openFromLibrary(video);
    }
  }, [useExternalPlayer, openFromLibrary, launchInPotPlayer, seekFor]);

  const handlePlayById = useCallback((videoId: string, position: number) => {
    const video = videos.find((v) => v.id === videoId);
    if (!video) return;
    if (useExternalPlayer) {
      launchInPotPlayer(video, position > 0 ? position : null);
    } else {
      openFromLibrary(video, position);
    }
  }, [videos, useExternalPlayer, openFromLibrary, launchInPotPlayer]);

  // 播放器内切换：沿用打开时的固定列表，只换当前视频
  const handlePlaylistSelect = useCallback((video: Video) => {
    openPlayer(video, progressMap[video.id] ?? 0);
  }, [openPlayer, progressMap]);

  const handlePlaylistDelete = useCallback(async (video: Video) => {
    if (!confirm(`确定要删除 "${video.filename}" 到回收站？`)) return;
    try {
      await api.deleteVideo(video.id);
    } catch (e) {
      alert(`删除失败：${String(e)}`);
      return;
    }
    const list = playlist ?? filteredVideos;
    const remaining = list.filter(v => v.id !== video.id);
    if (currentVideo?.id === video.id) {
      const idx = list.findIndex(v => v.id === video.id);
      const next = remaining[idx] ?? remaining[idx - 1];
      if (next) {
        setPlaylist(remaining);
        openPlayer(next, progressMap[next.id] ?? 0);
      } else {
        handleClosePlayer();
      }
    } else if (playlist) {
      setPlaylist(remaining);
    }
    loadVideos();
  }, [playlist, filteredVideos, currentVideo, progressMap, openPlayer, handleClosePlayer, loadVideos]);

  // 播放列表内移动：更新快照路径；若是当前播放项则按新路径从上次进度重新挂载
  const handlePlaylistMove = useCallback(async (video: Video) => {
    let dir: string | null;
    try {
      dir = await open({ directory: true, multiple: false, title: "选择目标文件夹" });
    } catch (err) {
      alert(`打开文件夹选择器失败: ${String(err)}`);
      return;
    }
    if (!dir) return;
    if (!confirm(`将 "${video.filename}" 移动到:\n${dir}`)) return;
    let newPath: string;
    try {
      newPath = await api.moveVideo(video.id, dir);
    } catch (err) {
      alert(`移动失败：${String(err)}`);
      return;
    }
    const moved = { ...video, path: newPath };
    setPlaylist(prev => prev ? prev.map(v => v.id === video.id ? moved : v) : prev);
    if (currentVideo?.id === video.id) {
      openPlayer(moved, progressMap[video.id] ?? 0);
    }
    loadVideos();
  }, [currentVideo, openPlayer, progressMap, loadVideos]);

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
        {(libraryError || notice) && (
          <div role="alert" className="bg-amber-900/80 text-amber-100 px-3 py-2 rounded text-sm flex justify-between items-center">
            <span>{libraryError || notice}</span>
            <button onClick={() => { clearLibraryError(); clearNotice(); }} aria-label="关闭提示" className="hover:text-white ml-2 shrink-0">✕</button>
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
          fakeCount={fakeIds.size}
          onConvertFakes={convertFakes}
          onClearFakes={clearFake}
          convertingFakes={converting}
          shortCount={shortIds.size}
          onConvertShorts={convertShorts}
          convertingShorts={convertingShorts}
          shortsDetected={shortsDetected}
          onDetectShorts={detectShorts}
          detectingShorts={detecting}
          onClearShorts={clearShorts}
          onGenerateThumbnails={handleGenerateThumbnails}
          generating={generating}
          thumbProgress={thumbProgress}
          withoutThumbnailCount={withoutThumbnailCount}
          onDetectHevc={handleDetectHevc}
          detectingHevc={detectingHevc}
          hevcDetected={hevcDetected}
          hevcCount={hevcCount}
          onConvertHevc={handleConvertHevc}
          convertingHevc={convertingHevc}
          hevcProgress={hevcProgress}
          onClearHevc={clearHevc}
          onDetectDuplicates={handleDetectDuplicates}
          detectingDuplicates={detectingDuplicates}
          duplicatesDetected={duplicatesDetected}
          duplicateGroupCount={duplicateGroupCount}
          duplicateExtrasCount={duplicateExtrasCount}
          onDeleteDuplicates={handleDeleteDuplicates}
          deletingDuplicates={deletingDuplicates}
          onClearDuplicates={clearDuplicates}
          selectMode={selectMode}
          selectedCount={selectedIds.size}
          allSelected={allSelected}
          onEnterSelect={() => setSelectMode(true)}
          onToggleSelectAll={toggleSelectAll}
          onDeleteSelected={handleDeleteSelected}
          deletingSelected={deletingSelected}
          onExitSelect={exitSelectMode}
        />
        <VideoGrid videos={filteredVideos} progressMap={progressMap} missingIds={missingIds} fakeIds={fakeIds} shortIds={shortIds} duplicateIds={duplicateIds} selectMode={selectMode} selectedIds={selectedIds} onToggleSelect={toggleSelect} onPlay={handlePlayVideo} onDeleted={loadVideos} onMoved={loadVideos} />
      </main>
      {!useExternalPlayer && currentVideo && (
        <PlayerView
          key={currentVideo.id}
          video={currentVideo}
          initialPosition={initialPosition}
          onClose={handleClosePlayer}
          onProgress={saveProgress}
          onFallback={handleFallbackToPotPlayer}
          playlist={playlist ?? undefined}
          playlistProgress={progressMap}
          onSelect={handlePlaylistSelect}
          onDelete={handlePlaylistDelete}
          onMove={handlePlaylistMove}
        />
      )}
      <ToastLayer toasts={toasts} onClose={dismiss} />
    </div>
  );
}

export default App;
