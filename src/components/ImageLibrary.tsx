import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { SearchBar } from "./SearchBar";
import { ImageGrid } from "./ImageGrid";
import { ImageToolbar } from "./ImageToolbar";
import type { ImageSortField } from "./ImageToolbar";
import { ImageViewer } from "./ImageViewer";
import { GroupReviewPanel } from "./GroupReviewPanel";
import { api } from "../api";
import { STALE_RESULT_NOTE } from "../detectionCache";
import type { Image } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useThumbnailGeneration } from "../hooks/useThumbnailGeneration";
import { useDuplicateGroups } from "../hooks/useDuplicateGroups";
import { useDeleteShortcut } from "../hooks/useDeleteShortcut";
import { useRangeSelect } from "../hooks/useRangeSelect";
import { useSimilarDetection } from "../hooks/useSimilarDetection";
import { filterMedia, selectedDirectoryLabel, sortMedia } from "../libraryFilter";
import type { SortDirection } from "../libraryFilter";

interface ImageLibraryProps {
  images: Image[];
  selectedDir: string | null;
  reloadImages: () => Promise<void>;
  /** 按 id 就地剔除本地清单（删除成功后用，省掉整库重拉） */
  dropLocally: (imageIds: string[]) => void;
  /** 移动成功后就地套用后端返回的新路径 */
  retargetLocally: (updates: Array<[imageId: string, newPath: string]>) => void;
  onScanDirectory: () => void;
  notify: Notify;
}

/** 图片库页面：与视频库各自的搜索、排序、多选与工具，共享的只是筛选/排序这类纯函数 */
export function ImageLibrary({ images, selectedDir, reloadImages, dropLocally, retargetLocally, onScanDirectory, notify }: ImageLibraryProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [sortField, setSortField] = useState<ImageSortField>("filename");
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [deletingSelected, setDeletingSelected] = useState(false);
  // 打开查看器时的列表快照：翻页范围固定，不受后续刷新影响
  const [viewerList, setViewerList] = useState<Image[]>([]);
  const [viewerIndex, setViewerIndex] = useState<number | null>(null);
  // 动图检测：多帧图片（GIF/APNG/动态 WebP/AVIF）命中后高亮，勾选进多选批量清理
  const [animatedIds, setAnimatedIds] = useState<Set<string>>(new Set());
  const [detectingAnimated, setDetectingAnimated] = useState(false);
  const [animatedDetected, setAnimatedDetected] = useState(false);

  const { thumbProgress, generating, generateAll } = useThumbnailGeneration(
    images, reloadImages, notify,
    { generate: api.generateImageThumbnails, event: "image-thumbnail-progress", unit: "图片" },
  );

  /** 一批图片进回收站，返回真正删掉的 id 并同步本地清单 */
  const trashImages = useCallback(async (imageIds: string[]) => {
    if (imageIds.length === 0) return [];
    const deleted = await api.deleteImages(imageIds);
    dropLocally(deleted);
    return deleted;
  }, [dropLocally]);

  // 重复图检测：后端边核对候选桶边推分组，面板逐组审阅，副本自动勾进多选
  const {
    groups: aliveDuplicateGroups,
    imageById: duplicateIndex,
    duplicateIds,
    groupCount: duplicateGroupCount,
    skipped: duplicateSkipped,
    stale: duplicateStale,
    detected: duplicatesDetected,
    detecting: detectingDuplicates,
    progress: duplicateProgress,
    panelOpen: duplicatePanelOpen,
    setPanelOpen: setDuplicatePanelOpen,
    detect: handleDetectDuplicates,
    clear: clearDuplicates,
    keep: keepDuplicate,
    autoSelect: autoSelectDuplicates,
    clearSelection: clearDuplicateSelection,
    setGroupSelection: selectDuplicateGroup,
  } = useDuplicateGroups({ images, setSelectMode, setSelectedIds, notify });

  // 相似图检测（pHash）：后端边算边推组，面板逐组审阅，新出现的副本自动勾进多选
  const {
    groups: aliveSimilarGroups,
    imageById: similarIndex,
    hashById: similarHashes,
    similarIds,
    groupCount: similarGroupCount,
    stale: similarStale,
    detected: similarDetected,
    detecting: detectingSimilar,
    progress: similarProgress,
    panelOpen: similarPanelOpen,
    setPanelOpen: setSimilarPanelOpen,
    detect: handleDetectSimilar,
    clear: clearSimilar,
    keep: keepSimilar,
    autoSelect: autoSelectSimilar,
    clearSelection: clearSimilarSelection,
    setGroupSelection: selectSimilarGroup,
    recalculating: similarRecalculating,
    threshold: similarThreshold,
    rethreshold: rethresholdSimilar,
    keepRule: similarKeepRule,
    applyKeepRule: applySimilarKeepRule,
  } = useSimilarDetection({ images, setSelectMode, setSelectedIds, notify });

  // 两个审阅面板都是全屏遮罩，只能开一个：开这一个就把另一个关掉，重跑检测也一样
  const openDuplicatePanel = useCallback(() => {
    setSimilarPanelOpen(false);
    setDuplicatePanelOpen(true);
  }, [setDuplicatePanelOpen, setSimilarPanelOpen]);
  const openSimilarPanel = useCallback(() => {
    setDuplicatePanelOpen(false);
    setSimilarPanelOpen(true);
  }, [setDuplicatePanelOpen, setSimilarPanelOpen]);
  const runDetectDuplicates = useCallback(() => {
    setSimilarPanelOpen(false);
    handleDetectDuplicates();
  }, [handleDetectDuplicates, setSimilarPanelOpen]);
  const runDetectSimilar = useCallback(() => {
    setDuplicatePanelOpen(false);
    handleDetectSimilar();
  }, [handleDetectSimilar, setDuplicatePanelOpen]);

  /** 面板标题后面那串小提示：恢复出来的旧结果要说清楚，免得被当成这一轮刚算的 */
  const duplicateCaveat = [
    duplicateSkipped > 0 ? `另有 ${duplicateSkipped} 张解不出画面，未参与比对` : "",
    duplicateStale ? STALE_RESULT_NOTE : "",
  ].filter(Boolean).join("；") || undefined;
  const similarCaveat = similarStale ? STALE_RESULT_NOTE : undefined;

  // 排一次、筛多次：切目录和打字只是从排好的清单里线性筛（19 万条 ≈30ms），
  // 不再每次条件一变就重排整库。Array.filter 保序，结果与"先筛后排"一致。
  const sortedImages = useMemo(
    () => sortMedia(images, sortField, sortDirection),
    [images, sortField, sortDirection],
  );
  const filteredImages = useMemo(
    () => filterMedia(sortedImages, selectedDir, searchQuery),
    [sortedImages, selectedDir, searchQuery],
  );

  // 筛选条件一变，勾选但已不在视图里的项不再可见，直接清空选择避免"隐形删除"
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedDir, searchQuery]);

  const toggleDirection = useCallback(() => {
    setSortDirection(prev => (prev === "asc" ? "desc" : "asc"));
  }, []);

  const { toggleSelect, selectAt } = useRangeSelect(filteredImages, setSelectedIds);

  const allSelected = filteredImages.length > 0 && filteredImages.every(i => selectedIds.has(i.id));
  const toggleSelectAll = useCallback(() => {
    setSelectedIds(allSelected ? new Set() : new Set(filteredImages.map(i => i.id)));
  }, [allSelected, filteredImages]);

  const exitSelectMode = useCallback(() => {
    setSelectMode(false);
    setSelectedIds(new Set());
  }, []);

  // 动图检测：后端按文件头数帧，命中即高亮；「勾选」把当前列表里的动图送进多选，
  // 删除仍复用「删除所选」那条通路（confirm + 回收站），不再另造一键删除
  const handleDetectAnimated = useCallback(async () => {
    setDetectingAnimated(true);
    try {
      const ids = await api.findAnimatedImages();
      setAnimatedIds(new Set(ids));
      setAnimatedDetected(true);
      if (ids.length === 0) notify("库里未检出动图。", "info");
    } catch (e) {
      notify(`检测动图失败：${String(e)}`, "error");
    } finally {
      setDetectingAnimated(false);
    }
  }, [notify]);

  const selectAnimated = useCallback(() => {
    const inView = filteredImages.filter(i => animatedIds.has(i.id));
    if (inView.length === 0) {
      notify("当前列表里没有动图（命中的都在其他目录或搜索结果之外）。", "info");
      return;
    }
    setSelectMode(true);
    setSelectedIds(new Set(inView.map(i => i.id)));
  }, [filteredImages, animatedIds, notify]);

  const clearAnimated = useCallback(() => {
    setAnimatedIds(new Set());
    setAnimatedDetected(false);
  }, []);

  // 删过之后 id 从库里消失，检测计数跟着收敛，别让工具栏一直报旧数
  useEffect(() => {
    setAnimatedIds(prev => {
      if (prev.size === 0) return prev;
      const alive = new Set(images.map(i => i.id));
      const next = new Set([...prev].filter(id => alive.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [images]);

  const handleDeleteSelected = useCallback(async () => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    if (!confirm(`确定将选中的 ${ids.length} 张图片移入回收站？`)) return;
    setDeletingSelected(true);
    let ok = 0;
    try {
      ok = (await trashImages(ids)).length;
    } catch (e) {
      notify(`删除失败：${String(e)}`, "error");
    }
    const failed = ids.length - ok;
    setSelectedIds(new Set());
    notify(failed > 0 ? `已删除 ${ok} 张，${failed} 张失败（可能被占用）` : `已将 ${ok} 张图片移入回收站。`, failed > 0 ? "error" : "info");
    setDeletingSelected(false);
  }, [selectedIds, trashImages, notify]);

  // Del 即删勾选；查看器盖在最上层时让位，免得在遮罩后批量删掉看不见的条目
  useDeleteShortcut(handleDeleteSelected,
    selectMode && selectedIds.size > 0 && viewerIndex === null && !deletingSelected);

  // 多选批量移动：选目录后逐个 move，失败只计数不中断
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
    if (!confirm(`将选中的 ${ids.length} 张图片移动到:\n${dir}`)) return;
    setMovingSelected(true);
    const moved: Array<[string, string]> = [];
    let failed = 0;
    for (const id of ids) {
      try {
        moved.push([id, await api.moveImage(id, dir)]);
      } catch {
        failed += 1;
      }
    }
    retargetLocally(moved);
    setSelectedIds(new Set());
    const ok = moved.length;
    notify(failed > 0 ? `已移动 ${ok} 张，${failed} 张失败（可能被占用或目标重名冲突）` : `已将 ${ok} 张图片移动到目标文件夹。`, failed > 0 ? "error" : "info");
    setMovingSelected(false);
  }, [selectedIds, retargetLocally, notify]);

  const openViewer = useCallback((image: Image) => {
    const index = filteredImages.findIndex(i => i.id === image.id);
    setViewerList(filteredImages);
    setViewerIndex(index >= 0 ? index : 0);
  }, [filteredImages]);

  /** 从分组面板放大：翻页范围就是这一组，逐张比对着决定删谁 */
  const openGroupInViewer = useCallback((image: Image, groupIds: string[], index: Map<string, Image>) => {
    const list = groupIds.map(id => index.get(id)).filter((item): item is Image => item !== undefined);
    if (list.length === 0) return;
    const at = list.findIndex(item => item.id === image.id);
    setViewerList(list);
    setViewerIndex(at >= 0 ? at : 0);
  }, []);
  const openSimilarGroup = useCallback(
    (image: Image, groupIds: string[]) => openGroupInViewer(image, groupIds, similarIndex),
    [openGroupInViewer, similarIndex],
  );
  const openDuplicateGroup = useCallback(
    (image: Image, groupIds: string[]) => openGroupInViewer(image, groupIds, duplicateIndex),
    [openGroupInViewer, duplicateIndex],
  );

  // 随机一张：直接以整个过滤结果为查看器列表，从随机位置开始看
  const handleRandomPick = useCallback(() => {
    if (filteredImages.length === 0) return;
    setViewerList(filteredImages);
    setViewerIndex(Math.floor(Math.random() * filteredImages.length));
  }, [filteredImages]);

  const closeViewer = useCallback(() => {
    setViewerIndex(null);
    setViewerList([]);
  }, []);

  // 查看器里删掉当前这张：就地接着看下一张，删完了才关闭
  const handleViewerDelete = useCallback(async (image: Image) => {
    const deleted = await trashImages([image.id]);
    if (deleted.length === 0) return;
    const remaining = viewerList.filter(i => i.id !== image.id);
    setViewerList(remaining);
    if (remaining.length === 0) setViewerIndex(null);
    else setViewerIndex(prev => Math.min(prev ?? 0, remaining.length - 1));
    notify("已将图片移入回收站。");
  }, [viewerList, trashImages, notify]);

  const withoutThumbnailCount = useMemo(
    () => images.filter(i => !i.thumbnail_path).length,
    [images],
  );

  return (
    <>
      <SearchBar
        value={searchQuery}
        onChange={setSearchQuery}
        total={filteredImages.length}
        title="图片库"
        unit="图片"
        measure="张"
        totalSize={filteredImages.reduce((s, i) => s + i.file_size, 0)}
      />
      <div className="flex justify-between items-center text-xs text-gray-400 shrink-0">
        <span className="truncate" title={selectedDir || "所有图片"}>{selectedDirectoryLabel(selectedDir, "所有图片")}</span>
        <span className="ml-3 shrink-0">{searchQuery ? "搜索结果" : "本地图片"}</span>
      </div>
      <ImageToolbar
        sortField={sortField}
        sortDirection={sortDirection}
        onSortFieldChange={setSortField}
        onToggleDirection={toggleDirection}
        onGenerateThumbnails={generateAll}
        generating={generating}
        thumbProgress={thumbProgress}
        withoutThumbnailCount={withoutThumbnailCount}
        onDetectDuplicates={runDetectDuplicates}
        detectingDuplicates={detectingDuplicates}
        duplicateProgress={duplicateProgress}
        duplicatesDetected={duplicatesDetected}
        duplicateGroupCount={duplicateGroupCount}
        onOpenDuplicateGroups={openDuplicatePanel}
        onClearDuplicates={clearDuplicates}
        onDetectSimilar={runDetectSimilar}
        detectingSimilar={detectingSimilar}
        similarDetected={similarDetected}
        similarGroupCount={similarGroupCount}
        similarProgress={similarProgress}
        onOpenSimilarGroups={openSimilarPanel}
        onClearSimilar={clearSimilar}
        onDetectAnimated={handleDetectAnimated}
        detectingAnimated={detectingAnimated}
        animatedDetected={animatedDetected}
        animatedCount={animatedIds.size}
        onSelectAnimated={selectAnimated}
        onClearAnimated={clearAnimated}
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
      />
      <ImageGrid
        images={filteredImages}
        duplicateIds={duplicateIds}
        similarIds={similarIds}
        animatedIds={animatedIds}
        selectMode={selectMode}
        selectedIds={selectedIds}
        onToggleSelect={selectAt}
        onOpen={openViewer}
        onScanDirectory={onScanDirectory}
        onDeleted={(imageId) => dropLocally([imageId])}
        onMoved={(imageId, newPath) => retargetLocally([[imageId, newPath]])}
        resetKey={selectedDir}
      />
      {duplicatePanelOpen && (
        <GroupReviewPanel
          noun="重复图片"
          progressLabel="核对候选"
          groups={aliveDuplicateGroups}
          imageById={duplicateIndex}
          selectedIds={selectedIds}
          onToggle={toggleSelect}
          onKeep={keepDuplicate}
          onAutoSelect={autoSelectDuplicates}
          onClearAll={clearDuplicateSelection}
          onSelectGroup={selectDuplicateGroup}
          selectedTotal={selectedIds.size}
          detecting={detectingDuplicates}
          progress={duplicateProgress}
          caveat={duplicateCaveat}
          onDeleteSelected={handleDeleteSelected}
          deleting={deletingSelected}
          onOpenImage={openDuplicateGroup}
          onClose={() => setDuplicatePanelOpen(false)}
        />
      )}
      {similarPanelOpen && (
        <GroupReviewPanel
          noun="相似图片"
          progressLabel="补算指纹"
          groups={aliveSimilarGroups}
          imageById={similarIndex}
          hashById={similarHashes}
          selectedIds={selectedIds}
          onToggle={toggleSelect}
          onKeep={keepSimilar}
          onAutoSelect={autoSelectSimilar}
          onClearAll={clearSimilarSelection}
          onSelectGroup={selectSimilarGroup}
          selectedTotal={selectedIds.size}
          detecting={detectingSimilar}
          recalculating={similarRecalculating}
          progress={similarProgress}
          caveat={similarCaveat}
          threshold={similarThreshold}
          onThreshold={rethresholdSimilar}
          keepRule={similarKeepRule}
          onKeepRule={applySimilarKeepRule}
          onDeleteSelected={handleDeleteSelected}
          deleting={deletingSelected}
          onOpenImage={openSimilarGroup}
          onClose={() => setSimilarPanelOpen(false)}
        />
      )}
      {viewerIndex !== null && viewerList[viewerIndex] && (
        <ImageViewer
          images={viewerList}
          index={viewerIndex}
          onNavigate={setViewerIndex}
          onClose={closeViewer}
          onDelete={handleViewerDelete}
        />
      )}
    </>
  );
}
