import { useState, useCallback, useEffect, lazy, Suspense } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { SearchBar } from "./components/SearchBar";
// 重面板懒加载：首屏只下视频库，图片库/播放器/历史按需拆包（JS 335KB 单包→多 chunk）
const ImageLibrary = lazy(() =>
  import("./components/ImageLibrary").then((m) => ({ default: m.ImageLibrary })),
);
const PlayerView = lazy(() =>
  import("./components/PlayerView").then((m) => ({ default: m.PlayerView })),
);
const PlayHistoryPanel = lazy(() =>
  import("./components/PlayHistoryPanel").then((m) => ({ default: m.PlayHistoryPanel })),
);
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
import { useDeleteShortcut, useMoveShortcut, useMoveNumberShortcut } from "./hooks/useDeleteShortcut";
import { useMoveTargets } from "./hooks/useMoveTargets";
import { useTauriEvent } from "./hooks/useTauriEvent";
import { useBatchSelection } from "./hooks/useBatchSelection";
import { useRangeSelect } from "./hooks/useRangeSelect";
import { useVideoLibrary } from "./hooks/useVideoLibrary";
import { api } from "./api";
import { STALE_RESULT_NOTE } from "./detectionCache";
import { loadScanRoots } from "./scanRootStore";
import { isUnderDir } from "./scanRoots";
import type { Image, MediaKind, ScanOutcome, Video } from "./types";
import type { DeletionReport } from "./api";
import type { SortField, SortDirection, WatchState } from "./libraryFilter";
import { selectedDirectoryLabel } from "./libraryFilter";
import { LibraryToolbar } from "./components/LibraryToolbar";
import { MoveTargetsDialog } from "./components/MoveTargetsDialog";
import { ToastLayer } from "./components/Toast";

/** 懒面板加载占位：与空库插画同风格，避免白屏 */
function PanelFallback({ label }: { label: string }) {
  return (
    <div className="flex-1 grid place-items-center text-gray-500" role="status" aria-live="polite">
      {label}加载中…
    </div>
  );
}

const POTPLAYER_PREF_KEY = "viewman.usePotPlayer";
const TAB_PREF_KEY = "viewman.mediaTab";
const SIDEBAR_VISIBLE_KEY = "viewman.sidebarVisible";

function App() {
  const { videos, progressMap, recentlyPlayed, loading, error: libraryError, clearError: clearLibraryError, scanDirectory, saveProgress, loadVideos, rescanStatus, applyScan: applyVideoScan, dropLocally: dropVideosLocally, retargetLocally: retargetVideosLocally } = useVideos();
  const {
    imageStats, libraryVersion, loading: imagesLoading, error: imagesError, clearError: clearImagesError,
    rescanStatus: imagesRescanStatus, scanDirectory: scanImageDirectory, loadImages, initialRun: loadImageLibrary,
    refresh: refreshImagesQuietly,
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
  // 视频侧搜索防抖：输入框即时响应，过滤用 250ms 后的快照（与图片库 ImageLibrary 同口径），
  // 19 万条下每次键击全量 filter+sort 不再掉帧
  const [debouncedQuery, setDebouncedQuery] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedQuery(searchQuery), 250);
    return () => clearTimeout(timer);
  }, [searchQuery]);
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

  // 勾选属于视频库：切到图片页就清空，不然图片页上按 Del/M 会命中看不见的视频
  useEffect(() => {
    setSelectedIds(new Set());
    setSelectMode(false);
  }, [tab]);
  // 播放器打开那一刻的列表快照：播放期间固定不变，不随库/排序/进度刷新而变
  const [playlist, setPlaylist] = useState<Video[] | null>(null);
  // 完整播放历史覆盖层
  const [historyOpen, setHistoryOpen] = useState(false);

  const { toasts, notify, dismiss } = useToasts();

  // 库自检：启动后跑一次 integrity_check。损坏多为 FTS 索引坏（删/搜时触发），
  // 自动无损重建一次；重建后仍坏就是主表坏，给出库文件路径手动恢复。
  // 文件本身不受影响：删库写一半炸了也只是部分条目没删，磁盘文件都在。
  const [dbIssue, setDbIssue] = useState<string | null>(null);
  const [repairing, setRepairing] = useState(false);
  const runRepair = useCallback(async () => {
    setRepairing(true);
    try {
      const rebuilt = await api.rebuildFtsIndexes();
      const recheck = await api.checkDbIntegrity();
      if (recheck === "ok") {
        setDbIssue(null);
        notify(`数据库索引已重建（${rebuilt}），删除/搜索恢复正常。`);
        await Promise.all([loadVideos(), loadImages()]);
      } else {
        const path = await api.dbFilePath().catch(() => "(未知路径)");
        setDbIssue(`主表损坏，重建索引无效：${recheck}。库文件在 ${path}，请从备份恢复该文件后重启（删库重扫是最后手段）。`);
      }
    } catch (e) {
      notify(`修复失败：${String(e)}`, "error");
    } finally {
      setRepairing(false);
    }
  }, [notify, loadVideos, loadImages]);
  useEffect(() => {
    let cancelled = false;
    api.checkDbIntegrity()
      .then(async (result) => {
        if (cancelled || result === "ok") return;
        // 先静默重建一次，八成是 FTS 索引坏；不行再亮红条
        try {
          await api.rebuildFtsIndexes();
          const recheck = await api.checkDbIntegrity();
          if (cancelled) return;
          if (recheck === "ok") {
            notify("检测到数据库索引损坏，已自动重建，无数据丢失。");
            return;
          }
          const path = await api.dbFilePath().catch(() => "(未知路径)");
          setDbIssue(`数据库主表损坏：${recheck}。库文件在 ${path}，请从备份恢复后重启。`);
        } catch (e) {
          if (!cancelled) setDbIssue(`数据库自检失败：${String(e)}`);
        }
      })
      .catch(() => { /* 自检失败不挡启动 */ });
    return () => { cancelled = true; };
  }, [notify]);
  const { scanProgress, resetScanProgress } = useScanProgress(rescanStatus, setNotice);
  const { scanProgress: imageScanProgress, resetScanProgress: resetImageScanProgress } =
    useScanProgress(imagesRescanStatus, setNotice, "image-scan-progress");
  const { thumbProgress, generating, generateAll: handleGenerateThumbnails } = useThumbnailGeneration(
    useCallback(() => Promise.resolve(videos.filter(v => !v.thumbnail_path).map(v => v.id)), [videos]),
    loadVideos, notify,
    { generate: api.generateThumbnails, resume: api.resumeVideoThumbnails, event: "thumbnail-progress", unit: "视频" },
  );
  const {
    hevcCount, hevcDetected, detecting: detectingHevc, detectProgress: hevcDetectProgress, detect: handleDetectHevc,
    converting: convertingHevc, convert: handleConvertHevc, hevcProgress, clearDetected: clearHevc,
  } = useHevcConversion(loadVideos, notify);
  /** 一批视频进回收站，返回三类清单并同步本地清单（与图片库同一条通路）。
   *  删库事务炸了也会抛错：finally 里重拉全库，保证界面与库真实一致，不留"删掉了还显示" */
  const trashVideos = useCallback(async (ids: string[]) => {
    if (ids.length === 0) return { deleted: [], stale: [], locked: [] };
    try {
      const report = await api.deleteVideos(ids);
      // 库记录已清的（删掉 + 路径失效）就地剔除；被锁的留着，下轮刷新/重试收敛
      dropVideosLocally([...report.deleted, ...report.stale]);
      return report;
    } catch (e) {
      await loadVideos();
      throw e;
    }
  }, [dropVideosLocally, loadVideos]);
  /** 作废视频侧的重复检测缓存：与图片侧同一个命令，只是换一份缓存文件 */
  const clearVideoDuplicateCache = useCallback(() => {
    api.clearDetectionCache("videoDuplicate").catch(() => { /* 清不掉只是重启后还能看到旧结果 */ });
  }, []);
  const {
    duplicateGroupCount, duplicateExtrasCount, duplicateIds, duplicatesDetected,
    detecting: detectingDuplicates, detect: handleDetectDuplicates,
    deleting: deletingDuplicates, deleteExtras: handleDeleteDuplicates, clear: clearDuplicates,
    progress: duplicateProgress, stale: duplicateStale,
  } = useDuplicates(notify, {
    detect: async () => ({ groups: await api.findDuplicateVideos() }),
    remove: trashVideos,
    unit: "视频",
    progressEvent: "video-duplicate-progress",
    // 上一趟的重复视频结果落盘存着：打开应用先恢复出来看，恢复的那份会照现在的库裁一遍
    liveItems: videos,
    loadCache: api.getVideoDuplicateCache,
    clearCache: clearVideoDuplicateCache,
  });
  /** 恢复出来的重复视频结果在工具栏上说清来源，免得被当成这一轮刚比对的 */
  const duplicateCaveat = duplicateStale ? STALE_RESULT_NOTE : undefined;
  const {
    missingIds, clearMissing,
    fakeIds, clearFake, convertFakes, converting,
    shortIds, convertShorts, convertingShorts, shortsDetected, detectShorts, detecting, clearShorts,
    checkProgress, checking, checkFiles: handleCheckFiles, cancelCheck: handleCancelCheck,
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

  // 目录监视自动扫描完成广播：后端把本轮的增删直接带过来，不再整库重拉
  useTauriEvent<ScanOutcome<Video>>("videos-changed", applyVideoScan);
  // 图片增量已由后端直接落库，前端只需要重拉统计与当前视图（不闪 loading）
  useTauriEvent<Image>("images-changed", () => void refreshImagesQuietly());

  /** 移除目录：只清除应用内的记录与封面缓存，磁盘文件保持原样（无确认，直接执行） */
  const handleRemoveRoot = useCallback(async (dir: string) => {
    const kind = tab;
    setRemovingRoot(true);
    try {
      const removed = await api.removeMediaDirectory(kind, dir);
      if (selectedDir && isUnderDir(selectedDir, dir)) setSelectedDir(null);
      if (selectedImageDir && isUnderDir(selectedImageDir, dir)) setSelectedImageDir(null);
      await (kind === "image" ? loadImages() : loadVideos());
      await refreshScanRoots();
      notify(removed > 0 ? `已移除 ${removed} 条记录，文件仍在磁盘上` : "已停止扫描该目录");
    } catch (e) {
      notify(`移除目录失败：${String(e)}`, "error");
    } finally {
      setRemovingRoot(false);
    }
  }, [tab, selectedDir, selectedImageDir, loadImages, loadVideos, refreshScanRoots, notify]);

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

  const handleFallbackToPotPlayer = useCallback((video: Video, position: number) => {
    handleClosePlayer();
    // position 来自播放器当前帧（保存进度之后传入），比 progressMap 快照新
    launchInPotPlayer(video, position > 0 ? position : seekFor(video));
  }, [handleClosePlayer, launchInPotPlayer, seekFor]);

  const toggleSortDirection = useCallback(() => {
    setSortDirection(prev => (prev === "asc" ? "desc" : "asc"));
  }, []);

  // 视频库派生逻辑已抽到 useVideoLibrary：App 只保留搜索防抖与选择状态，过滤/排序/聚合不再内联
  const { filteredVideos, videoTotals, withoutThumbnailCount } = useVideoLibrary(videos, {
    selectedDir,
    searchQuery: debouncedQuery,
    progressMap,
    watchState,
    minSizeGb,
    minDurationMin,
    minHeight,
    sortField,
    sortDirection,
  });

  // 筛选条件一变，之前勾选但已不在视图里的项不再可见，直接清空选择避免"隐形删除"
  // 注意用防抖后的 query：输入过程中不清空已选，停稳后才按最终视图收敛
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedDir, debouncedQuery, watchState, minSizeGb, minDurationMin, minHeight]);

  const { selectAt: selectVideoAt } = useRangeSelect(filteredVideos, setSelectedIds);

  const {
    allSelected, toggleSelectAll, exitSelectMode,
    deletingSelected, movingSelected, moveProgress, cancelMove,
    movePrompt, setMovePrompt,
    handleDeleteSelected, handleMoveSelected, moveSelectedTo,
  } = useBatchSelection({
    view: filteredVideos,
    selectedIds, setSelectedIds, setSelectMode,
    notify,
    noun: "视频", measure: "个", kind: "video",
    trash: trashVideos,
    describe: async (ids) => {
      const byId = new Map(videos.map(v => [v.id, v.filename] as const));
      return ids.map(id => byId.get(id) ?? id);
    },
    moveOne: (id, dir) => api.moveVideo(id, dir),
    afterMove: (moved) => retargetVideosLocally(moved),
  });

  // 移动目标清单共享缓存：对话框与数字键直达共用一份，弹窗秒开、数字键不用等加载
  const { targets: videoMoveTargets } = useMoveTargets("video");

  // Del 即删勾选：只要有待删清单就生效，不再要求多选模式（分组面板里点卡片勾选不经过 selectMode）；
  // 播放器或历史面板盖在上面时让位
  useDeleteShortcut(handleDeleteSelected,
    selectedIds.size > 0 && !currentVideo && !historyOpen && !deletingSelected && movePrompt === null);

  // M 即移动勾选：与 Del 同一套门禁（播放器/历史面板/移动对话框盖在上面时让位）
  useMoveShortcut(handleMoveSelected,
    selectedIds.size > 0 && !currentVideo && !historyOpen && !movingSelected && movePrompt === null);

  // 数字键 1-9 直达：勾选后按 1/2/3… 直接移到清单对应目录，免弹清单。门禁同 M。
  useMoveNumberShortcut(videoMoveTargets, (dir) => {
    void moveSelectedTo(dir);
  }, selectedIds.size > 0 && !currentVideo && !historyOpen && !movingSelected && movePrompt === null);

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
    // 删的是正在播的这条：先关播放器放掉文件句柄，否则回收站报"被占用"删不动
    if (currentVideo?.id === video.id) closePlayer();
    let report: DeletionReport;
    try {
      report = await trashVideos([video.id]);
    } catch (e) {
      notify(`删除失败：${String(e)}`, "error");
      return;
    }
    if (report.locked.length > 0) {
      notify(`删除失败：${video.filename} 被占用（关闭占用它的程序后重试）`, "error");
      return;
    }
    if (report.stale.length > 0) {
      notify(`记录路径已不在磁盘，仅清理了记录：${video.filename}`, "info");
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
  }, [playlist, filteredVideos, currentVideo, progressMap, openPlayer, handleClosePlayer, closePlayer, trashVideos]);

  // 播放列表内移动：弹目标目录列表；更新快照路径，若是当前播放项则按新路径从上次进度重新挂载
  const handlePlaylistMove = useCallback(async (video: Video) => {
    setMovePrompt({
      kind: "video",
      noun: "视频",
      count: 1,
      onPick: async (dir) => {
        // 移的是正在播的这条：先关播放器放掉句柄，否则 rename 撞锁失败
        if (currentVideo?.id === video.id) closePlayer();
        let newPath: string;
        try {
          newPath = await api.moveVideo(video.id, dir);
        } catch (err) {
          notify(`移动失败：${String(err)}`, "error");
          return false;
        }
        const moved = { ...video, path: newPath };
        setPlaylist(prev => prev ? prev.map(v => v.id === video.id ? moved : v) : prev);
        if (currentVideo?.id === video.id) {
          openPlayer(moved, progressMap[video.id] ?? 0);
        }
        retargetVideosLocally([[video.id, newPath]]);
        return true;
      },
    });
  }, [currentVideo, closePlayer, openPlayer, progressMap, retargetVideosLocally, notify]);

  const activeError = tab === "image" ? imagesError : libraryError;
  const clearActiveError = tab === "image" ? clearImagesError : clearLibraryError;

  // 左侧目录栏显隐开关，记住用户上次的选择
  const [sidebarVisible, setSidebarVisible] = useState(() => localStorage.getItem(SIDEBAR_VISIBLE_KEY) !== "0");
  const toggleSidebar = useCallback(() => {
    setSidebarVisible(v => {
      localStorage.setItem(SIDEBAR_VISIBLE_KEY, v ? "0" : "1");
      return !v;
    });
  }, []);

  return (
    <div className="library-shell h-screen w-screen flex text-white overflow-hidden">
      {sidebarVisible && (
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
        images={imageStats}
        selectedImageDir={selectedImageDir}
        onSelectImageDir={setSelectedImageDir}
        onScanImageDirectory={handleImageScan}
        imageLoading={imagesLoading}
        imageScanProgress={imageScanProgress}
        imageRescanStatus={imagesRescanStatus}
        roots={scanRoots}
        removingRoot={removingRoot}
        onRemoveRoot={handleRemoveRoot}
        onCancelScan={(task) => {
          api.cancelTask(task).catch((e) => notify(`取消失败：${String(e)}`, "error"));
        }}
      />
      )}
      <button
        onClick={toggleSidebar}
        className="shrink-0 w-7 self-stretch grid place-items-center text-gray-500 hover:text-white hover:bg-white/5 transition"
        title={sidebarVisible ? "隐藏侧栏" : "显示侧栏"}
        aria-label={sidebarVisible ? "隐藏侧栏" : "显示侧栏"}
      >
        {sidebarVisible ? "‹" : "›"}
      </button>
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
        {dbIssue && (
          <div role="alert" className="bg-red-900/80 text-red-100 px-3 py-2 rounded text-sm flex justify-between items-center gap-3">
            <span className="break-all">{dbIssue}</span>
            <button
              onClick={() => void runRepair()}
              disabled={repairing}
              className="shrink-0 rounded-lg border border-white/20 bg-white/10 px-3 py-1 hover:bg-white/20 disabled:opacity-50"
            >
              {repairing ? "修复中…" : "重新修复"}
            </button>
          </div>
        )}
        {tab === "image" ? (
          <Suspense fallback={<PanelFallback label="图片库" />}>
            <ImageLibrary
              libraryVersion={libraryVersion}
              stats={imageStats}
              refreshLibrary={refreshImagesQuietly}
              selectedDir={selectedImageDir}
              onScanDirectory={handlePickImageDirectory}
              notify={notify}
            />
          </Suspense>
        ) : (
          <>
            <SearchBar value={searchQuery} onChange={setSearchQuery} total={filteredVideos.length} title="视频库" unit="视频" totalDuration={videoTotals.totalDuration} totalSize={videoTotals.totalSize} />
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
              onCancelCheck={handleCancelCheck}
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
              duplicateProgress={duplicateProgress}
              duplicatesDetected={duplicatesDetected}
              duplicateGroupCount={duplicateGroupCount}
              duplicateExtrasCount={duplicateExtrasCount}
              onDeleteDuplicates={handleDeleteDuplicates}
              deletingDuplicates={deletingDuplicates}
              onClearDuplicates={clearDuplicates}
              duplicateCaveat={duplicateCaveat}
              selectMode={selectMode}
              selectedCount={selectedIds.size}
              allSelected={allSelected}
              onEnterSelect={() => setSelectMode(true)}
              onToggleSelectAll={toggleSelectAll}
              onDeleteSelected={handleDeleteSelected}
              deletingSelected={deletingSelected}
              onMoveSelected={handleMoveSelected}
              movingSelected={movingSelected}
              moveProgress={moveProgress}
              onCancelMove={cancelMove}
              onExitSelect={exitSelectMode}
              onRandomPick={handleRandomPick}
              onOpenHistory={() => setHistoryOpen(true)}
            />
            <VideoGrid videos={filteredVideos} progressMap={progressMap} missingIds={missingIds} fakeIds={fakeIds} shortIds={shortIds} duplicateIds={duplicateIds} selectMode={selectMode} selectedIds={selectedIds} onToggleSelect={selectVideoAt} onPlay={handlePlayVideo} onDeleted={(videoId) => dropVideosLocally([videoId])} onMoved={(videoId, newPath) => retargetVideosLocally([[videoId, newPath]])} resetKey={selectedDir} />
          </>
        )}
      </main>
      {!useExternalPlayer && currentVideo && (
        <Suspense fallback={<PanelFallback label="播放器" />}>
          <PlayerView
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
            onError={(message) => notify(message, "error")}
          />
        </Suspense>
      )}
      {historyOpen && (
        <Suspense fallback={<PanelFallback label="播放历史" />}>
          <PlayHistoryPanel
            onClose={() => setHistoryOpen(false)}
            onPlay={(videoId, position) => {
              setHistoryOpen(false);
              handlePlayById(videoId, position);
            }}
          />
        </Suspense>
      )}
      {movePrompt && (
        <MoveTargetsDialog
          kind={movePrompt.kind}
          noun={movePrompt.noun}
          count={movePrompt.count}
          onPick={movePrompt.onPick}
          onClose={() => setMovePrompt(null)}
        />
      )}
      <ToastLayer toasts={toasts} onClose={dismiss} />
    </div>
  );
}

export default App;
