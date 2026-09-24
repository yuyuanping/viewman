import { useState, useMemo, useCallback, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { ImageLibrary } from "./components/ImageLibrary";
import { PlayerView } from "./components/PlayerView";
import { SearchBar } from "./components/SearchBar";
import { useVideos } from "./hooks/useVideos";
import { useImages } from "./hooks/useImages";
import { usePlayer } from "./hooks/usePlayer";
import { usePotPlayer } from "./hooks/usePotPlayer";
import { useToasts } from "./hooks/useToasts";
import { useScanProgress } from "./hooks/useScanProgress";
import { useThumbnailGeneration } from "./hooks/useThumbnailGeneration";
import { useHevcConversion } from "./hooks/useHevcConversion";
import { useDuplicates } from "./hooks/useDuplicates";
import { useFileCheck } from "./hooks/useFileCheck";
import { api } from "./api";
import { loadScanRoots } from "./scanRootStore";
import { countUnderDir, isUnderDir } from "./scanRoots";
import type { MediaKind, Video } from "./types";
import type { SortField, SortDirection, WatchState } from "./libraryFilter";
import { filterMedia, filterByWatchState, filterByMedia, sortMedia, selectedDirectoryLabel } from "./libraryFilter";
import { LibraryToolbar } from "./components/LibraryToolbar";
import { PlayHistoryPanel } from "./components/PlayHistoryPanel";
import { ToastLayer } from "./components/Toast";

const POTPLAYER_PREF_KEY = "viewman.usePotPlayer";
const TAB_PREF_KEY = "viewman.mediaTab";

function App() {
  const { videos, progressMap, recentlyPlayed, loading, error: libraryError, clearError: clearLibraryError, scanDirectory, saveProgress, loadVideos, rescanStatus, dropLocally: dropVideosLocally, retargetLocally: retargetVideosLocally } = useVideos();
  const {
    images, loading: imagesLoading, error: imagesError, clearError: clearImagesError,
    rescanStatus: imagesRescanStatus, scanDirectory: scanImageDirectory, loadImages, initialRun: loadImageLibrary,
    dropLocally: dropImagesLocally, retargetLocally: retargetImagesLocally,
  } = useImages();
  const { currentVideo, initialPosition, openPlayer, closePlayer } = usePlayer();
  const { launch: launchInPotPlayer, error: potPlayerError, clearError: clearPotPlayerError } = usePotPlayer(saveProgress);

  const [tab, setTab] = useState<MediaKind>(() => (localStorage.getItem(TAB_PREF_KEY) === "image" ? "image" : "video"));
  const changeTab = useCallback((next: MediaKind) => {
    setTab(next);
    localStorage.setItem(TAB_PREF_KEY, next);
  }, []);
  const [selectedImageDir, setSelectedImageDir] = useState<string | null>(null);

  useEffect(() => {
    void loadImageLibrary();
  }, [loadImageLibrary]);

  const [searchQuery, setSearchQuery] = useState("");
  const [selectedDir, setSelectedDir] = useState<string | null>(null);
  const [useExternalPlayer, setUseExternalPlayer] = useState(() => localStorage.getItem(POTPLAYER_PREF_KEY) === "1");
  const [notice, setNotice] = useState<string | null>(null);
  const clearNotice = useCallback(() => setNotice(null), []);
  const [sortField, setSortField] = useState<SortField>("filename");
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [watchState, setWatchState] = useState<WatchState>("all");
  // 高级过滤：null=不限；数值为下限
  const [minSizeGb, setMinSizeGb] = useState<string>("");   // "" | "1" | "5"（GB）
  const [minDurationMin, setMinDurationMin] = useState<string>(""); // "" | "10" | "30" | "60"（分钟）
  const [minHeight, setMinHeight] = useState<string>("");   // "" | "480" | "720" | "1080" | "2160"（p）
  // 多选批量删除模式
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [deletingSelected, setDeletingSelected] = useState(false);
  // 播放器打开那一刻的列表快照：播放期间固定不变，不随库/排序/进度刷新而变
  const [playlist, setPlaylist] = useState<Video[] | null>(null);
  // 完整播放历史覆盖层
  const [historyOpen, setHistoryOpen] = useState(false);

  const { toasts, notify, dismiss } = useToasts();
  const { scanProgress, resetScanProgress } = useScanProgress(rescanStatus, setNotice);
  const { scanProgress: imageScanProgress, resetScanProgress: resetImageScanProgress } =
    useScanProgress(imagesRescanStatus, setNotice, "image-scan-progress");
  const { thumbProgress, generating, generateAll: handleGenerateThumbnails } = useThumbnailGeneration(
    videos, loadVideos, notify,
    { generate: api.generateThumbnails, event: "thumbnail-progress", unit: "视频" },
  );
  const {
    hevcCount, hevcDetected, detecting: detectingHevc, detectProgress: hevcDetectProgress, detect: handleDetectHevc,
    converting: convertingHevc, convert: handleConvertHevc, hevcProgress, clearDetected: clearHevc,
  } = useHevcConversion(loadVideos, notify);
  /** 一批视频进回收站，返回真正删掉的 id 并同步本地清单（与图片库同一条通路） */
  const trashVideos = useCallback(async (ids: string[]) => {
    if (ids.length === 0) return [];
    const deleted = await api.deleteVideos(ids);
    dropVideosLocally(deleted);
    return deleted;
  }, [dropVideosLocally]);
  const {
    duplicateGroupCount, duplicateExtrasCount, duplicateIds, duplicatesDetected,
    detecting: detectingDuplicates, detect: handleDetectDuplicates,
    deleting: deletingDuplicates, deleteExtras: handleDeleteDuplicates, clear: clearDuplicates,
  } = useDuplicates(notify, { detect: api.findDuplicateVideos, remove: trashVideos, unit: "视频" });
  const {
    missingIds, clearMissing,
    fakeIds, clearFake, convertFakes, converting,
    shortIds, convertShorts, convertingShorts, shortsDetected, detectShorts, detecting, clearShorts,
    checkProgress, checking, checkFiles: handleCheckFiles,
  } = useFileCheck(videos, setNotice, loadVideos);

  // 侧栏"扫描目录"清单：只看当前标签页那一份，扫描或移除之后要重新读
  const [scanRoots, setScanRoots] = useState<string[]>([]);
  const [removingRoot, setRemovingRoot] = useState(false);
  const refreshScanRoots = useCallback(async () => {
    setScanRoots(await loadScanRoots(tab));
  }, [tab]);

  useEffect(() => {
    void refreshScanRoots();
  }, [refreshScanRoots]);

  // 目录监视自动扫描完成广播：后台重扫了哪个库就刷新哪个库
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<string>("library-changed", (e) => {
      const kind = e.payload;
      if (kind === "video") void loadVideos();
      else if (kind === "image") void loadImages();
    }).then(fn => { if (disposed) fn(); else unlisten = fn; })
      .catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, [loadVideos, loadImages]);

  /** 移除目录：只清除应用内的记录与封面缓存，磁盘文件保持原样 */
  const handleRemoveRoot = useCallback(async (dir: string) => {
    const kind = tab;
    const count = countUnderDir(kind === "image" ? images : videos, dir);
    const detail = count > 0
      ? `将清除该目录下 ${count} 个条目的库内记录（磁盘上的文件不会被删除），并停止自动扫描。`
      : "库内没有挂在它下面的条目，将只停止自动扫描。";
    if (!confirm(`移除目录？\n${dir}\n\n${detail}`)) return;
    setRemovingRoot(true);
    try {
      const removed = await api.removeMediaDirectory(kind, dir);
      if (selectedDir && isUnderDir(selectedDir, dir)) setSelectedDir(null);
      if (selectedImageDir && isUnderDir(selectedImageDir, dir)) setSelectedImageDir(null);
      await (kind === "image" ? loadImages() : loadVideos());
      await refreshScanRoots();
      notify(removed > 0 ? `已移除 ${removed} 条记录，文件仍在磁盘上` : "已停止扫描该目录");
    } finally {
      setRemovingRoot(false);
    }
  }, [tab, images, videos, selectedDir, selectedImageDir, loadImages, loadVideos, refreshScanRoots, notify]);

  const handleScan = useCallback(async (dir: string) => {
    resetScanProgress();
    setNotice(null);
    try {
      await scanDirectory(dir);
      await refreshScanRoots();
    } finally {
      resetScanProgress();
    }
  }, [scanDirectory, resetScanProgress, refreshScanRoots]);

  const handleImageScan = useCallback(async (dir: string) => {
    resetImageScanProgress();
    setNotice(null);
    try {
      await scanImageDirectory(dir);
      await refreshScanRoots();
    } finally {
      resetImageScanProgress();
    }
  }, [scanImageDirectory, resetImageScanProgress, refreshScanRoots]);

  // 图片库空列表里的扫描入口：自己弹目录选择器
  const handlePickImageDirectory = useCallback(async () => {
    try {
      const dir = await open({ directory: true, multiple: false, title: "选择图片目录" });
      if (dir) await handleImageScan(dir);
    } catch (e) {
      notify(`无法扫描目录：${String(e)}`, "error");
    }
  }, [handleImageScan, notify]);

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
    const matched = filterMedia(videos, selectedDir, searchQuery);
    const byWatchState = filterByWatchState(matched, progressMap, (v) => v.id, watchState);
    const mediaFilter = {
      minSize: minSizeGb ? Number(minSizeGb) * 1024 ** 3 : null,
      minDuration: minDurationMin ? Number(minDurationMin) * 60 : null,
      minHeight: minHeight ? Number(minHeight) : null,
    };
    const byMedia = filterByMedia(byWatchState, mediaFilter);
    return sortMedia(byMedia, sortField, sortDirection);
  }, [videos, selectedDir, searchQuery, progressMap, watchState, minSizeGb, minDurationMin, minHeight, sortField, sortDirection]);

  // 筛选条件一变，之前勾选但已不在视图里的项不再可见，直接清空选择避免"隐形删除"
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedDir, searchQuery, watchState, minSizeGb, minDurationMin, minHeight]);

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
    try {
      ok = (await trashVideos(ids)).length;
    } catch (e) {
      notify(`删除失败：${String(e)}`, "error");
    }
    const failed = ids.length - ok;
    setSelectedIds(new Set());
    notify(failed > 0 ? `已删除 ${ok} 个，${failed} 个失败（可能被占用）` : `已将 ${ok} 个视频移入回收站。`, failed > 0 ? "error" : "info");
    setDeletingSelected(false);
  }, [selectedIds, trashVideos, notify]);

  // 多选批量移动：选目录后逐个 move，失败只计数不中断（被占用的文件跳过）
  const [movingSelected, setMovingSelected] = useState(false);
  const handleMoveSelected = useCallback(async () => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    let dir: string | null;
    try {
      dir = await open({ directory: true, multiple: false, title: "选择目标文件夹" });
    } catch {
      return;
    }
    if (!dir) return;
    if (!confirm(`将选中的 ${ids.length} 个视频移动到:\n${dir}`)) return;
    setMovingSelected(true);
    const moved: Array<[string, string]> = [];
    let failed = 0;
    for (const id of ids) {
      try {
        moved.push([id, await api.moveVideo(id, dir)]);
      } catch {
        failed += 1;
      }
    }
    retargetVideosLocally(moved);
    setSelectedIds(new Set());
    const ok = moved.length;
    notify(failed > 0 ? `已移动 ${ok} 个，${failed} 个失败（可能被占用或目标重名冲突）` : `已将 ${ok} 个视频移动到目标文件夹。`, failed > 0 ? "error" : "info");
    setMovingSelected(false);
  }, [selectedIds, retargetVideosLocally, notify]);

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

  // 随机播放：从当前过滤结果里抽一个（跳过假视频）
  const handleRandomPick = useCallback(() => {
    const pool = filteredVideos.filter(v => !fakeIds.has(v.id));
    if (pool.length === 0) return;
    handlePlayVideo(pool[Math.floor(Math.random() * pool.length)]);
  }, [filteredVideos, fakeIds, handlePlayVideo]);

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
    let deleted: string[];
    try {
      deleted = await trashVideos([video.id]);
    } catch (e) {
      alert(`删除失败：${String(e)}`);
      return;
    }
    if (deleted.length === 0) {
      alert("删除失败：文件可能被占用");
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
  }, [playlist, filteredVideos, currentVideo, progressMap, openPlayer, handleClosePlayer, trashVideos]);

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
    retargetVideosLocally([[video.id, newPath]]);
  }, [currentVideo, openPlayer, progressMap, retargetVideosLocally]);

  const activeError = tab === "image" ? imagesError : libraryError;
  const clearActiveError = tab === "image" ? clearImagesError : clearLibraryError;

  return (
    <div className="library-shell h-screen w-screen flex text-white overflow-hidden">
      <Sidebar
        tab={tab}
        onTabChange={changeTab}
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
        images={images}
        selectedImageDir={selectedImageDir}
        onSelectImageDir={setSelectedImageDir}
        onScanImageDirectory={handleImageScan}
        imageLoading={imagesLoading}
        imageScanProgress={imageScanProgress}
        imageRescanStatus={imagesRescanStatus}
        roots={scanRoots}
        removingRoot={removingRoot}
        onRemoveRoot={handleRemoveRoot}
      />
      <main className="library-main flex-1 flex flex-col gap-5 overflow-hidden">
        {(activeError || notice) && (
          <div role="alert" className="bg-amber-900/80 text-amber-100 px-3 py-2 rounded text-sm flex justify-between items-center">
            <span>{activeError || notice}</span>
            <button onClick={() => { clearActiveError(); clearNotice(); }} aria-label="关闭提示" className="hover:text-white ml-2 shrink-0">✕</button>
          </div>
        )}
        {potPlayerError && (
          <div className="bg-red-900/80 text-red-200 px-3 py-2 rounded text-sm flex justify-between items-center">
            <span>{potPlayerError}</span>
            <button onClick={clearPotPlayerError} className="text-red-300 hover:text-white ml-2">✕</button>
          </div>
        )}
        {tab === "image" ? (
          <ImageLibrary
            images={images}
            selectedDir={selectedImageDir}
            reloadImages={loadImages}
            dropLocally={dropImagesLocally}
            retargetLocally={retargetImagesLocally}
            onScanDirectory={handlePickImageDirectory}
            notify={notify}
          />
        ) : (
          <>
            <SearchBar value={searchQuery} onChange={setSearchQuery} total={filteredVideos.length} title="视频库" unit="视频" totalDuration={filteredVideos.reduce((s, v) => s + (v.duration ?? 0), 0)} totalSize={filteredVideos.reduce((s, v) => s + v.file_size, 0)} />
            <div className="flex justify-between items-center text-xs text-gray-400 shrink-0"><span className="truncate" title={selectedDir || "所有视频"}>{selectedDirectoryLabel(selectedDir)}</span><span className="ml-3 shrink-0">{searchQuery ? "搜索结果" : "本地媒体"}</span></div>
            <LibraryToolbar
              sortField={sortField}
              sortDirection={sortDirection}
              onSortFieldChange={setSortField}
              onToggleDirection={toggleSortDirection}
              watchState={watchState}
              onWatchStateChange={setWatchState}
              minSizeGb={minSizeGb}
              minDurationMin={minDurationMin}
              minHeight={minHeight}
              onMinSizeGbChange={setMinSizeGb}
              onMinDurationMinChange={setMinDurationMin}
              onMinHeightChange={setMinHeight}
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
              hevcDetectProgress={hevcDetectProgress}
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
              onMoveSelected={handleMoveSelected}
              movingSelected={movingSelected}
              onExitSelect={exitSelectMode}
              onRandomPick={handleRandomPick}
              onOpenHistory={() => setHistoryOpen(true)}
            />
            <VideoGrid videos={filteredVideos} progressMap={progressMap} missingIds={missingIds} fakeIds={fakeIds} shortIds={shortIds} duplicateIds={duplicateIds} selectMode={selectMode} selectedIds={selectedIds} onToggleSelect={toggleSelect} onPlay={handlePlayVideo} onDeleted={(videoId) => dropVideosLocally([videoId])} onMoved={(videoId, newPath) => retargetVideosLocally([[videoId, newPath]])} resetKey={selectedDir} />
          </>
        )}
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
      {historyOpen && (
        <PlayHistoryPanel
          onClose={() => setHistoryOpen(false)}
          onPlay={(videoId, position) => {
            setHistoryOpen(false);
            handlePlayById(videoId, position);
          }}
        />
      )}
      <ToastLayer toasts={toasts} onClose={dismiss} />
    </div>
  );
}

export default App;
