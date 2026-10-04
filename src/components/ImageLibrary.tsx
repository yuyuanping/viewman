import { useCallback, useEffect, useMemo, useState } from "react";
import { SearchBar } from "./SearchBar";
import { ImageGrid } from "./ImageGrid";
import { ImageToolbar } from "./ImageToolbar";
import type { ImageSortField } from "./ImageToolbar";
import { ImageViewer } from "./ImageViewer";
import { GroupReviewPanel } from "./GroupReviewPanel";
import { TemplateMatchPanel } from "./TemplateMatchPanel";
import { MoveTargetsDialog } from "./MoveTargetsDialog";
import { api } from "../api";
import type { ImageStatsPayload } from "../api";
import { STALE_RESULT_NOTE } from "../detectionCache";
import type { Image } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useThumbnailGeneration } from "../hooks/useThumbnailGeneration";
import { useDuplicateGroups } from "../hooks/useDuplicateGroups";
import { useDeleteShortcut, useMoveShortcut, useMoveNumberShortcut } from "../hooks/useDeleteShortcut";
import { useMoveTargets } from "../hooks/useMoveTargets";
import { useRangeSelect } from "../hooks/useRangeSelect";
import { useBatchSelection } from "../hooks/useBatchSelection";
import { useTemplateSearch } from "../hooks/useTemplateSearch";
import { selectedDirectoryLabel, sortMedia } from "../libraryFilter";
import type { SortDirection } from "../libraryFilter";

interface ImageLibraryProps {
  /** 库内容代次：扫描/删除/移动之后由数据层推进，这里据此重拉当前视图 */
  libraryVersion: number;
  /** 侧栏聚合统计：缺封面计数从这来 */
  stats: ImageStatsPayload | null;
  /** 库内容变了之后的重拉（统计在数据层，视图在这里） */
  refreshLibrary: () => Promise<void>;
  selectedDir: string | null;
  onScanDirectory: () => void;
  notify: Notify;
}

/** 搜索词防抖：19 万级库里每次键击都打后端没必要，停手 250ms 再查 */
const SEARCH_DEBOUNCE_MS = 250;

/** 图片库页面：与视频库各自的搜索、排序、多选与工具，共享的只是筛选/排序这类纯函数 */
export function ImageLibrary({ libraryVersion, stats, refreshLibrary, selectedDir, onScanDirectory, notify }: ImageLibraryProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [sortField, setSortField] = useState<ImageSortField>("filename");
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  // 打开查看器时的列表快照：翻页范围固定，不受后续刷新影响
  const [viewerList, setViewerList] = useState<Image[]>([]);
  const [viewerIndex, setViewerIndex] = useState<number | null>(null);
  // 当前视图（后端按目录+搜索过滤后的行）与它的聚合计数
  const [view, setView] = useState<Image[]>([]);
  const [viewStats, setViewStats] = useState<{ total: number; totalSize: number }>({ total: 0, totalSize: 0 });
  // 动图检测：多帧图片（GIF/APNG/动态 WebP/AVIF）命中后高亮，勾选进多选批量清理
  const [animatedIds, setAnimatedIds] = useState<Set<string>>(new Set());
  const [detectingAnimated, setDetectingAnimated] = useState(false);
  const [animatedDetected, setAnimatedDetected] = useState(false);

  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(searchQuery), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [searchQuery]);

  // 视图拉取：目录/搜索/代次任一变化就重查。cancelled 标记挡住竞态——
  // 慢的旧响应回来不能盖掉新的
  useEffect(() => {
    let cancelled = false;
    api.getImageView(selectedDir, debouncedSearch)
      .then(res => {
        if (cancelled) return;
        setView(res.items);
        setViewStats({ total: res.total, totalSize: res.total_size });
      })
      .catch(e => notify(`读取图片列表失败：${String(e)}`, "error"));
    return () => { cancelled = true; };
  }, [selectedDir, debouncedSearch, libraryVersion, notify]);

  const { thumbProgress, generating, generateAll } = useThumbnailGeneration(
    () => api.getMissingImageThumbnailIds(), refreshLibrary, notify,
    { generate: api.generateImageThumbnails, resume: api.resumeImageThumbnails, event: "image-thumbnail-progress", unit: "图片" },
  );

  /** 一批图片进回收站，返回三类清单；无论成败都刷新视图与统计，
   *  让界面与库真实内容一致——删库事务炸了会抛错，跳过刷新就留下"删掉了还显示"的旧账 */
  const trashImages = useCallback(async (imageIds: string[]) => {
    if (imageIds.length === 0) return { deleted: [], stale: [], locked: [] };
    try {
      return await api.deleteImages(imageIds);
    } finally {
      await refreshLibrary();
    }
  }, [refreshLibrary]);

  // 比对像素开关：记忆上次选择，重复检测快检/精检两种模式
  const [pixelVerify, setPixelVerify] = useState<boolean>(
    () => localStorage.getItem("viewman.duplicatePixelVerify") !== "0",
  );
  const changePixelVerify = useCallback((next: boolean) => {
    setPixelVerify(next);
    localStorage.setItem("viewman.duplicatePixelVerify", next ? "1" : "0");
  }, []);

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
    selectAll: selectAllDuplicates,
    clearSelection: clearDuplicateSelection,
    setGroupSelection: selectDuplicateGroup,
    cachePixelVerify: duplicateCachePixelVerify,
  } = useDuplicateGroups({ fetchImagesByIds: api.getImagesByIds, libraryTotal: stats?.total ?? 0, libraryVersion, setSelectMode, setSelectedIds, notify, pixelVerify });

  // 模板匹配（以图搜图）：右键/查看器里选一张图当模板，命中按距离排成一份清单
  const {
    template: templateImage,
    matches: templateMatches,
    imageById: templateIndex,
    skipped: templateSkipped,
    threshold: templateThreshold,
    setThreshold: setTemplateThreshold,
    searching: templateSearching,
    progress: templateProgress,
    panelOpen: templatePanelOpen,
    search: searchByTemplate,
    clear: clearTemplate,
    selectIds: selectTemplateIds,
  } = useTemplateSearch({ fetchImagesByIds: api.getImagesByIds, libraryVersion, setSelectMode, setSelectedIds, notify });

  // 审阅面板是全屏遮罩：重跑检测时把别的面板关掉
  const openDuplicatePanel = useCallback(() => {
    setDuplicatePanelOpen(true);
  }, [setDuplicatePanelOpen]);
  const runDetectDuplicates = useCallback(() => {
    handleDetectDuplicates();
  }, [handleDetectDuplicates]);

  // 模板匹配入口：结果面板也是全屏遮罩，发起时把别的面板和查看器都收掉
  const runTemplateSearch = useCallback((image: Image) => {
    setViewerIndex(null);
    setDuplicatePanelOpen(false);
    void searchByTemplate(image);
  }, [searchByTemplate, setDuplicatePanelOpen]);

  // 扩展名修正：把"解不出画面"里内容与扩展名不符的图就地改名（后端同步库记录，
  // 不必重扫），改完自动重跑重复检测，让这批图真的参与比对
  const [fixingExtensions, setFixingExtensions] = useState(false);
  const handleFixExtensions = useCallback(async () => {
    setFixingExtensions(true);
    try {
      const report = await api.fixMismatchedExtensions();
      const parts = [`改名 ${report.renamed} 张`];
      if (report.failed) parts.push(`${report.failed} 张没改成（可能被占用）`);
      if (report.unrecognized) parts.push(`${report.unrecognized} 张认不出格式，原样保留`);
      notify(`扩展名修正完成：${parts.join("，")}。正在重跑重复检测…`);
      handleDetectDuplicates();
    } catch (e) {
      notify(`修正扩展名失败：${String(e)}`, "error");
    } finally {
      setFixingExtensions(false);
    }
  }, [handleDetectDuplicates, notify]);

  /** 面板标题后面那串小提示：恢复出来的旧结果要说清楚，免得被当成这一轮刚算的 */
  const duplicateCaveat = [
    duplicateSkipped > 0 ? `另有 ${duplicateSkipped} 张解不出画面，未参与比对` : "",
    duplicateStale ? STALE_RESULT_NOTE : "",
    // 缓存那份用的模式跟当前开关不一致：直接看会误判精度，提示重跑一次
    duplicateCachePixelVerify !== null && duplicateCachePixelVerify !== pixelVerify
      ? `这份结果是「比对像素${duplicateCachePixelVerify ? "开" : "关"}」模式下跑的，当前开关是「${pixelVerify ? "开" : "关"}」，重跑一次为准`
      : "",
  ].filter(Boolean).join("；") || undefined;

  // 目录+搜索过滤在后端做完了，前端只排一次序。排序留在前端：
  // SQLite 没有等价于 Intl.Collator 的中文拼音排序，硬下推会改变排序语义
  const filteredImages = useMemo(
    () => sortMedia(view, sortField, sortDirection),
    [view, sortField, sortDirection],
  );

  // 筛选条件一变，勾选但已不在视图里的项不再可见，直接清空选择避免"隐形删除"
  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectedDir, debouncedSearch]);

  const toggleDirection = useCallback(() => {
    setSortDirection(prev => (prev === "asc" ? "desc" : "asc"));
  }, []);

  const { toggleSelect, selectAt } = useRangeSelect(filteredImages, setSelectedIds);

  // Shift 连选不要求先点「多选」：Shift 点下去就直接进多选，工具栏的删除/移动立即跟上
  const handleGridToggle = useCallback((index: number, shiftKey: boolean) => {
    if (shiftKey) setSelectMode(true);
    selectAt(index, shiftKey);
  }, [selectAt]);

  const {
    allSelected, toggleSelectAll, exitSelectMode,
    deletingSelected, movingSelected, movePrompt, setMovePrompt,
    handleDeleteSelected, handleMoveSelected, moveSelectedTo,
  } = useBatchSelection({
    view: filteredImages,
    selectedIds, setSelectedIds, setSelectMode,
    notify,
    noun: "图片", measure: "张", kind: "image",
    trash: trashImages,
    // 删不掉时点名：视图 + 各检测面板的元数据索引先查，查不到的现查后端
    describe: async (ids) => {
      const names: string[] = [];
      const missing: string[] = [];
      for (const id of ids) {
        const hit = filteredImages.find(i => i.id === id)
          ?? duplicateIndex.get(id) ?? templateIndex.get(id);
        if (hit) names.push(hit.filename);
        else missing.push(id);
      }
      if (missing.length > 0) {
        try {
          const rows = await api.getImagesByIds(missing);
          const byId = new Map(rows.map(r => [r.id, r.filename] as const));
          for (const id of missing) names.push(byId.get(id) ?? id);
        } catch {
          names.push(...missing);
        }
      }
      return names;
    },
    moveOne: async (id, dir) => { await api.moveImage(id, dir); return id; },
    afterMove: () => refreshLibrary(),
  });

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

  // 删过之后 id 从库里消失，检测计数跟着收敛，别让工具栏一直报旧数。
  // 库里没有全量清单了，"还活着吗"交给后端按 id 查；库代次一动就复核一遍
  useEffect(() => {
    if (animatedIds.size === 0) return;
    let disposed = false;
    api.getImagesByIds([...animatedIds]).then(rows => {
      if (disposed) return;
      const alive = new Set(rows.map(r => r.id));
      setAnimatedIds(prev => {
        const next = new Set([...prev].filter(id => alive.has(id)));
        return next.size === prev.size ? prev : next;
      });
    }).catch(() => { /* 查不动就先留着旧计数 */ });
    return () => { disposed = true; };
  }, [libraryVersion, animatedIds]);

  // Del 即删勾选：只要有待删清单就生效，不再要求多选模式——分组面板里点卡片勾选
  // 不经过 selectMode，旧条件会让 Del 在面板里静默失灵。查看器或移动对话框盖在
  // 最上层时让位，免得在遮罩后批量删掉看不见的条目
  useDeleteShortcut(handleDeleteSelected,
    selectedIds.size > 0 && viewerIndex === null && !deletingSelected && movePrompt === null);

  // M 即移动勾选：与 Del 同一套门禁（查看器或移动对话框盖在上面时让位，查看器里 M 移动的是当前这张）
  useMoveShortcut(handleMoveSelected,
    selectedIds.size > 0 && viewerIndex === null && !movingSelected && movePrompt === null);

  // 数字键 1-9 直达：勾选后按 1/2/3… 直接移到清单对应目录，免弹清单。门禁同 M。
  const { targets: imageMoveTargets } = useMoveTargets("image");
  useMoveNumberShortcut(imageMoveTargets, (dir) => {
    void moveSelectedTo(dir);
  }, selectedIds.size > 0 && viewerIndex === null && !movingSelected && movePrompt === null);

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

  // 单张删除（面板/查看器右键共用）：返回值表示是否真删掉了，调用方据此决定翻页
  const handleSingleDelete = useCallback(async (image: Image): Promise<boolean> => {
    let report;
    try {
      report = await trashImages([image.id]);
    } catch (e) {
      notify(`删除失败：${String(e)}`, "error");
      return false;
    }
    if (report.locked.length > 0) {
      notify(`删除失败：${image.filename} 被占用（关闭占用它的程序后重试）`, "error");
      return false;
    }
    // 这张若此前被手动勾在多选里，删掉后从勾选集剔除，别让死 id 混进"删除所选"虚报失败数
    setSelectedIds(prev => {
      if (!prev.has(image.id)) return prev;
      const next = new Set(prev);
      next.delete(image.id);
      return next;
    });
    notify(report.stale.length > 0 ? "记录路径已不在磁盘，仅清理了记录。" : "已将图片移入回收站。");
    return true;
  }, [trashImages, notify, setSelectedIds]);

  // 查看器里删掉当前这张：就地接着看下一张，删完了才关闭
  const handleViewerDelete = useCallback(async (image: Image) => {
    if (!await handleSingleDelete(image)) return;
    const remaining = viewerList.filter(i => i.id !== image.id);
    setViewerList(remaining);
    if (remaining.length === 0) setViewerIndex(null);
    else setViewerIndex(prev => Math.min(prev ?? 0, remaining.length - 1));
  }, [viewerList, handleSingleDelete]);

  // 单张移动（面板右键共用）：复用批量那套移动对话框，移完刷新并清理勾选
  const handleSingleMove = useCallback(async (image: Image) => {
    setMovePrompt({
      kind: "image",
      noun: "图片",
      count: 1,
      onPick: async (dir) => {
        try {
          await api.moveImage(image.id, dir);
        } catch (e) {
          notify(`移动失败：${String(e)}`, "error");
          return false;
        }
        await refreshLibrary();
        setSelectedIds(prev => {
          if (!prev.has(image.id)) return prev;
          const next = new Set(prev);
          next.delete(image.id);
          return next;
        });
        notify("已移动图片。");
        return true;
      },
    });
  }, [refreshLibrary, notify, setSelectedIds]);

  // 查看器里移动当前这张（M）：弹出目标目录列表，选定后直接移动；移完图已不在当前
  // 视图口径里，与删除一样就地接着看下一张。移动失败时不翻页
  const handleViewerMove = useCallback(async (image: Image) => {
    setMovePrompt({
      kind: "image",
      noun: "图片",
      count: 1,
      onPick: async (dir) => {
        try {
          await api.moveImage(image.id, dir);
        } catch (e) {
          notify(`移动失败：${String(e)}`, "error");
          return false;
        }
        await refreshLibrary();
        setSelectedIds(prev => {
          if (!prev.has(image.id)) return prev;
          const next = new Set(prev);
          next.delete(image.id);
          return next;
        });
        const remaining = viewerList.filter(i => i.id !== image.id);
        setViewerList(remaining);
        if (remaining.length === 0) setViewerIndex(null);
        else setViewerIndex(prev => Math.min(prev ?? 0, remaining.length - 1));
        notify("已移动图片。");
        return true;
      },
    });
  }, [viewerList, refreshLibrary, notify, setSelectedIds]);

  return (
    <>
      <SearchBar
        value={searchQuery}
        onChange={setSearchQuery}
        total={viewStats.total}
        title="图片库"
        unit="图片"
        measure="张"
        totalSize={viewStats.totalSize}
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
        withoutThumbnailCount={stats?.missing_thumbnails ?? 0}
        onDetectDuplicates={runDetectDuplicates}
        detectingDuplicates={detectingDuplicates}
        duplicateProgress={duplicateProgress}
        duplicatesDetected={duplicatesDetected}
        duplicateGroupCount={duplicateGroupCount}
        onOpenDuplicateGroups={openDuplicatePanel}
        onClearDuplicates={clearDuplicates}
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
        animatedIds={animatedIds}
        selectMode={selectMode}
        selectedIds={selectedIds}
        onToggleSelect={handleGridToggle}
        onOpen={openViewer}
        onScanDirectory={onScanDirectory}
        onDeleted={() => refreshLibrary()}
        onMoved={() => refreshLibrary()}
        onFindSimilar={runTemplateSearch}
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
          onSelectAll={selectAllDuplicates}
          onClearAll={clearDuplicateSelection}
          onSelectGroup={selectDuplicateGroup}
          selectedTotal={selectedIds.size}
          detecting={detectingDuplicates}
          progress={duplicateProgress}
          caveat={duplicateCaveat}
          pixelVerify={pixelVerify}
          onPixelVerify={changePixelVerify}
          onDeleteSelected={handleDeleteSelected}
          deleting={deletingSelected}
          onFixExtensions={handleFixExtensions}
          fixingExtensions={fixingExtensions}
          onOpenImage={openDuplicateGroup}
          onMoveImage={handleSingleMove}
          onDeleteImage={handleSingleDelete}
          onError={(message) => notify(message, "error")}
          onClose={() => setDuplicatePanelOpen(false)}
        />
      )}
      {templatePanelOpen && (
        <TemplateMatchPanel
          template={templateImage}
          matches={templateMatches}
          imageById={templateIndex}
          skipped={templateSkipped}
          threshold={templateThreshold}
          onThreshold={setTemplateThreshold}
          searching={templateSearching}
          progress={templateProgress}
          selectedIds={selectedIds}
          onToggle={toggleSelect}
          onSelectIds={selectTemplateIds}
          onClearAll={() => setSelectedIds(new Set())}
          selectedTotal={selectedIds.size}
          onDeleteSelected={handleDeleteSelected}
          deleting={deletingSelected}
          onMoveSelected={handleMoveSelected}
          moving={movingSelected}
          onOpenImage={(image, matchIds) => openGroupInViewer(image, matchIds, templateIndex)}
          onMoveImage={handleSingleMove}
          onDeleteImage={handleSingleDelete}
          onError={(message) => notify(message, "error")}
          onClose={clearTemplate}
        />
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
      {viewerIndex !== null && viewerList[viewerIndex] && (
        <ImageViewer
          images={viewerList}
          index={viewerIndex}
          onNavigate={setViewerIndex}
          onClose={closeViewer}
          onDelete={handleViewerDelete}
          onMove={handleViewerMove}
          onFindSimilar={runTemplateSearch}
          onError={(message) => notify(message, "error")}
        />
      )}
    </>
  );
}
