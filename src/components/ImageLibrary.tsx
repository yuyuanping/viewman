import { useCallback, useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { SearchBar } from "./SearchBar";
import { ImageGrid } from "./ImageGrid";
import { ImageToolbar } from "./ImageToolbar";
import type { ImageSortField } from "./ImageToolbar";
import { ImageViewer } from "./ImageViewer";
import { api } from "../api";
import type { Image } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useThumbnailGeneration } from "../hooks/useThumbnailGeneration";
import { useDuplicates } from "../hooks/useDuplicates";
import { filterMedia, selectedDirectoryLabel, sortMedia } from "../libraryFilter";
import type { SortDirection } from "../libraryFilter";

interface ImageLibraryProps {
  images: Image[];
  selectedDir: string | null;
  reloadImages: () => Promise<void>;
  onScanDirectory: () => void;
  notify: Notify;
}

/** 图片库页面：与视频库各自的搜索、排序、多选与工具，共享的只是筛选/排序这类纯函数 */
export function ImageLibrary({ images, selectedDir, reloadImages, onScanDirectory, notify }: ImageLibraryProps) {
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
  const {
    duplicateGroupCount, duplicateExtrasCount, duplicateIds, duplicatesDetected,
    detecting: detectingDuplicates, detect: handleDetectDuplicates,
    deleting: deletingDuplicates, deleteExtras: handleDeleteDuplicates, clear: clearDuplicates,
  } = useDuplicates(reloadImages, notify, { detect: api.findDuplicateImages, remove: api.deleteImage, unit: "图片" });

  const filteredImages = useMemo(
    () => sortMedia(filterMedia(images, selectedDir, searchQuery), sortField, sortDirection),
    [images, selectedDir, searchQuery, sortField, sortDirection],
  );

  // 筛选条件一变，勾选但已不在视图里的项不再可见，直接清空选择避免"隐形删除"
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedDir, searchQuery]);

  const toggleDirection = useCallback(() => {
    setSortDirection(prev => (prev === "asc" ? "desc" : "asc"));
  }, []);

  const toggleSelect = useCallback((image: Image) => {
    setSelectedIds(prev => {
      const next = new Set(prev);
      if (next.has(image.id)) next.delete(image.id); else next.add(image.id);
      return next;
    });
  }, []);

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
    let failed = 0;
    for (const id of ids) {
      try {
        await api.deleteImage(id);
        ok += 1;
      } catch {
        failed += 1;
      }
    }
    setSelectedIds(new Set());
    await reloadImages();
    notify(failed > 0 ? `已删除 ${ok} 张，${failed} 张失败（可能被占用）` : `已将 ${ok} 张图片移入回收站。`, failed > 0 ? "error" : "info");
    setDeletingSelected(false);
  }, [selectedIds, reloadImages, notify]);

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
    let ok = 0;
    let failed = 0;
    for (const id of ids) {
      try {
        await api.moveImage(id, dir);
        ok += 1;
      } catch {
        failed += 1;
      }
    }
    setSelectedIds(new Set());
    await reloadImages();
    notify(failed > 0 ? `已移动 ${ok} 张，${failed} 张失败（可能被占用或目标重名冲突）` : `已将 ${ok} 张图片移动到目标文件夹。`, failed > 0 ? "error" : "info");
    setMovingSelected(false);
  }, [selectedIds, reloadImages, notify]);

  const openViewer = useCallback((image: Image) => {
    const index = filteredImages.findIndex(i => i.id === image.id);
    setViewerList(filteredImages);
    setViewerIndex(index >= 0 ? index : 0);
  }, [filteredImages]);

  const closeViewer = useCallback(() => {
    setViewerIndex(null);
    setViewerList([]);
  }, []);

  // 查看器里删掉当前这张：就地接着看下一张，删完了才关闭
  const handleViewerDelete = useCallback(async (image: Image) => {
    await api.deleteImage(image.id);
    await reloadImages();
    const remaining = viewerList.filter(i => i.id !== image.id);
    setViewerList(remaining);
    if (remaining.length === 0) setViewerIndex(null);
    else setViewerIndex(prev => Math.min(prev ?? 0, remaining.length - 1));
    notify("已将图片移入回收站。");
  }, [viewerList, reloadImages, notify]);

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
      />
      <ImageGrid
        images={filteredImages}
        duplicateIds={duplicateIds}
        selectMode={selectMode}
        selectedIds={selectedIds}
        onToggleSelect={toggleSelect}
        onOpen={openViewer}
        onScanDirectory={onScanDirectory}
        onDeleted={reloadImages}
        onMoved={reloadImages}
      />
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
