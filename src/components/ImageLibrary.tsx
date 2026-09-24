import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { SearchBar } from "./SearchBar";
import { ImageGrid } from "./ImageGrid";
import { ImageToolbar } from "./ImageToolbar";
import type { ImageSortField } from "./ImageToolbar";
import { ImageViewer } from "./ImageViewer";
import { SimilarGroupsPanel } from "./SimilarGroupsPanel";
import { api } from "../api";
import type { Image } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useThumbnailGeneration } from "../hooks/useThumbnailGeneration";
import { useDuplicates } from "../hooks/useDuplicates";
import { useDeleteShortcut } from "../hooks/useDeleteShortcut";
import { useRangeSelect } from "../hooks/useRangeSelect";
import { filterMedia, selectedDirectoryLabel, sortMedia } from "../libraryFilter";
import { extrasOfGroups, liveGroups, selectGroupExtras } from "../similarGroups";
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

  const {
    duplicateGroupCount, duplicateExtrasCount, duplicateIds, duplicatesDetected,
    detecting: detectingDuplicates, detect: handleDetectDuplicates,
    deleting: deletingDuplicates, deleteExtras: handleDeleteDuplicates, clear: clearDuplicates,
  } = useDuplicates(notify, { detect: api.findDuplicateImages, remove: trashImages, unit: "图片" });

  // 相似图检测（pHash）：结果单独成面板按组审阅，检测完自动进多选并勾上各组副本
  const [similarGroups, setSimilarGroups] = useState<string[][]>([]);
  const [similarKeeps, setSimilarKeeps] = useState<Record<number, string>>({});
  const [similarDetected, setSimilarDetected] = useState(false);
  const [similarPanelOpen, setSimilarPanelOpen] = useState(false);
  const [detectingSimilar, setDetectingSimilar] = useState(false);

  /** 只索引相似组里那几百张，避免为 19 万条清单建一张全库 Map */
  const similarIndex = useMemo(() => {
    const wanted = new Set(similarGroups.flat());
    if (wanted.size === 0) return new Map<string, Image>();
    const byId = new Map<string, Image>();
    for (const image of images) if (wanted.has(image.id)) byId.set(image.id, image);
    return byId;
  }, [images, similarGroups]);

  const aliveSimilarGroups = useMemo(
    () => liveGroups(similarGroups, new Set(similarIndex.keys()), similarKeeps),
    [similarGroups, similarIndex, similarKeeps],
  );
  const similarExtras = useMemo(() => extrasOfGroups(aliveSimilarGroups), [aliveSimilarGroups]);

  /** 每组保留首张（最早入库那张），其余勾上 */
  const autoSelectSimilar = useCallback(() => {
    setSelectMode(true);
    setSelectedIds(new Set(similarExtras));
  }, [similarExtras]);

  const handleDetectSimilar = useCallback(async () => {
    setDetectingSimilar(true);
    try {
      const found = await api.findSimilarImages();
      if (found.length === 0) {
        setSimilarDetected(false);
        notify("没有发现相似的图片系列。");
        return;
      }
      const alive = new Set<string>();
      for (const image of images) alive.add(image.id);
      const extras = extrasOfGroups(liveGroups(found, alive, {}));
      setSimilarGroups(found);
      setSimilarKeeps({});
      setSimilarDetected(true);
      setSelectMode(true);
      setSelectedIds(new Set(extras));
      setSimilarPanelOpen(true);
      notify(`发现 ${found.length} 组相似图片，已自动勾上 ${extras.length} 个副本。`);
    } catch (e) {
      notify(`相似检测失败：${String(e)}`, "error");
    } finally {
      setDetectingSimilar(false);
    }
  }, [images, notify]);

  const clearSimilar = useCallback(() => {
    setSimilarGroups([]);
    setSimilarKeeps({});
    setSimilarDetected(false);
    setSimilarPanelOpen(false);
  }, []);

  /** 换某组的保留项：该组勾选跟着翻转，其他组的勾选不动 */
  const keepSimilar = useCallback((at: number, keepId: string) => {
    const group = aliveSimilarGroups.find(item => item.at === at);
    if (!group) return;
    setSimilarKeeps(prev => ({ ...prev, [at]: keepId }));
    setSelectedIds(prev => selectGroupExtras(prev, group, keepId));
  }, [aliveSimilarGroups]);

  const similarIds = useMemo(
    () => new Set(aliveSimilarGroups.flatMap(group => group.ids)),
    [aliveSimilarGroups],
  );

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
  const openSimilarGroup = useCallback((image: Image, groupIds: string[]) => {
    const list = groupIds.map(id => similarIndex.get(id)).filter((item): item is Image => item !== undefined);
    if (list.length === 0) return;
    const at = list.findIndex(item => item.id === image.id);
    setViewerList(list);
    setViewerIndex(at >= 0 ? at : 0);
  }, [similarIndex]);

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
        onDetectDuplicates={handleDetectDuplicates}
        detectingDuplicates={detectingDuplicates}
        duplicatesDetected={duplicatesDetected}
        duplicateGroupCount={duplicateGroupCount}
        duplicateExtrasCount={duplicateExtrasCount}
        onDeleteDuplicates={handleDeleteDuplicates}
        deletingDuplicates={deletingDuplicates}
        onClearDuplicates={clearDuplicates}
        onDetectSimilar={handleDetectSimilar}
        detectingSimilar={detectingSimilar}
        similarDetected={similarDetected}
        similarGroupCount={aliveSimilarGroups.length}
        onOpenSimilarGroups={() => setSimilarPanelOpen(true)}
        onClearSimilar={clearSimilar}
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
        selectMode={selectMode}
        selectedIds={selectedIds}
        onToggleSelect={selectAt}
        onOpen={openViewer}
        onScanDirectory={onScanDirectory}
        onDeleted={(imageId) => dropLocally([imageId])}
        onMoved={(imageId, newPath) => retargetLocally([[imageId, newPath]])}
        resetKey={selectedDir}
      />
      {similarPanelOpen && (
        <SimilarGroupsPanel
          groups={aliveSimilarGroups}
          imageById={similarIndex}
          selectedIds={selectedIds}
          onToggle={toggleSelect}
          onKeep={keepSimilar}
          onAutoSelect={autoSelectSimilar}
          selectedTotal={selectedIds.size}
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
