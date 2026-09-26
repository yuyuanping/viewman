import type { SortDirection } from "../libraryFilter";

/** 图片库没有时长，排序字段比视频库少一项 */
export type ImageSortField = "filename" | "file_size" | "created_at" | "modified_at";

interface ImageToolbarProps {
  sortField: ImageSortField;
  sortDirection: SortDirection;
  onSortFieldChange: (field: ImageSortField) => void;
  onToggleDirection: () => void;
  onGenerateThumbnails: () => void;
  generating: boolean;
  thumbProgress: { processed: number; total: number } | null;
  withoutThumbnailCount: number;
  onDetectDuplicates: () => void;
  detectingDuplicates: boolean;
  /** 判据改成解码指纹后要逐张跑 ffmpeg，进度得报出来 */
  duplicateProgress: { processed: number; total: number; stage: string } | null;
  duplicatesDetected: boolean;
  duplicateGroupCount: number;
  /** 重复组也走分组面板审阅：删除按「删除所选」/Del 那条通路，不再一键清空副本 */
  onOpenDuplicateGroups: () => void;
  onClearDuplicates: () => void;
  /** 相似图检测（pHash）：分组面板审阅，副本自动勾进多选 */
  onDetectSimilar: () => void;
  detectingSimilar: boolean;
  similarDetected: boolean;
  similarGroupCount: number;
  /** 检测进行中已比对的张数，边算边出组时用来显示进度 */
  similarProgress: { processed: number; total: number } | null;
  onOpenSimilarGroups: () => void;
  onClearSimilar: () => void;
  /** 动图检测：文件头数帧，命中高亮，勾选后走「删除所选」批量清理 */
  onDetectAnimated: () => void;
  detectingAnimated: boolean;
  animatedDetected: boolean;
  animatedCount: number;
  onSelectAnimated: () => void;
  onClearAnimated: () => void;
  selectMode: boolean;
  selectedCount: number;
  allSelected: boolean;
  onEnterSelect: () => void;
  onToggleSelectAll: () => void;
  onDeleteSelected: () => void;
  deletingSelected: boolean;
  onMoveSelected: () => void;
  movingSelected: boolean;
  onExitSelect: () => void;
  /** 从当前过滤结果里随机打开一张 */
  onRandomPick: () => void;
}

/** 后端 duplicate-progress 的两趟活儿：补指纹按张数报，核对候选按候选桶报 */
const DUP_STAGES: Record<string, string> = { sigs: "补指纹", grouping: "核对候选" };

const SORT_OPTIONS: { value: ImageSortField; label: string }[] = [
  { value: "filename", label: "文件名" },
  { value: "file_size", label: "文件大小" },
  { value: "created_at", label: "添加时间" },
  { value: "modified_at", label: "修改时间" },
];

export function ImageToolbar({
  sortField,
  sortDirection,
  onSortFieldChange,
  onToggleDirection,
  onGenerateThumbnails,
  generating,
  thumbProgress,
  withoutThumbnailCount,
  onDetectDuplicates,
  detectingDuplicates,
  duplicateProgress,
  duplicatesDetected,
  duplicateGroupCount,
  onOpenDuplicateGroups,
  onClearDuplicates,
  onDetectSimilar,
  detectingSimilar,
  similarDetected,
  similarGroupCount,
  similarProgress,
  onOpenSimilarGroups,
  onClearSimilar,
  onDetectAnimated,
  detectingAnimated,
  animatedDetected,
  animatedCount,
  onSelectAnimated,
  onClearAnimated,
  selectMode,
  selectedCount,
  allSelected,
  onEnterSelect,
  onToggleSelectAll,
  onDeleteSelected,
  deletingSelected,
  onMoveSelected,
  movingSelected,
  onExitSelect,
  onRandomPick,
}: ImageToolbarProps) {
  return (
    <div className="library-toolbar shrink-0">
      <span className="toolbar-label">排序</span>
      <select
        className="toolbar-select"
        value={sortField}
        onChange={(e) => onSortFieldChange(e.target.value as ImageSortField)}
        aria-label="排序字段"
      >
        {SORT_OPTIONS.map(option => (
          <option key={option.value} value={option.value}>{option.label}</option>
        ))}
      </select>
      <button
        type="button"
        onClick={onToggleDirection}
        className="toolbar-chip"
        aria-label={sortDirection === "asc" ? "当前升序，点击切换为降序" : "当前降序，点击切换为升序"}
        title={sortDirection === "asc" ? "升序" : "降序"}
      >
        {sortDirection === "asc" ? "↑ 升序" : "↓ 降序"}
      </button>
      <button
        type="button"
        className="toolbar-chip"
        onClick={onRandomPick}
        title="从当前列表随机打开一张"
      >
        随机一张
      </button>

      <span className="toolbar-spacer" />

      {/* 审完/删空后组数归零，检测入口要能重新出现 */}
      {(!duplicatesDetected || duplicateGroupCount === 0) && !detectingDuplicates && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectDuplicates}
          disabled={detectingDuplicates}
          title="按解码后的图像内容比对，找出内容相同的重复图片：重存、转格式过的那张也算同一张（不再比文件字节）"
        >
          检测重复图片
        </button>
      )}
      {/* 检测还在跑就把面板关掉时，工具栏必须留着入口：进度和已出的组都在里面 */}
      {detectingDuplicates && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onOpenDuplicateGroups}
          title="检测仍在进行，打开面板看进度和已经定下的组"
        >
          {`比对中…${duplicateProgress ? ` ${DUP_STAGES[duplicateProgress.stage] ?? "比对"} ${duplicateProgress.processed}/${duplicateProgress.total}` : ""} · 查看`}
        </button>
      )}
      {duplicatesDetected && duplicateGroupCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onOpenDuplicateGroups}
            title="逐组查看重复图片，可改保留哪一张"
          >
            {duplicateGroupCount} 组重复 · 查看
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearDuplicates} title="清除重复检测结果">
            ✕
          </button>
        </>
      )}
      {(!similarDetected || similarGroupCount === 0) && !detectingSimilar && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectSimilar}
          disabled={detectingSimilar}
          title="感知哈希比对，找出相似但不相同的连拍/截图系列；每组保留最早一张，其余自动勾进多选"
        >
          {detectingSimilar
            ? `比对中…${similarProgress ? ` ${similarProgress.processed}/${similarProgress.total}` : ""}`
            : "检测相似图片"}
        </button>
      )}
      {/* 检测还在跑就把面板关掉时，工具栏必须留着入口：进度和已出的组都在里面 */}
      {detectingSimilar && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onOpenSimilarGroups}
          title="检测仍在进行，打开面板看进度和已经出来的组"
        >
          {`比对中…${similarProgress ? ` ${similarProgress.processed}/${similarProgress.total}` : ""} · 查看`}
        </button>
      )}
      {similarDetected && similarGroupCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onOpenSimilarGroups}
            title="逐组查看相似图片，可改保留哪一张"
          >
            {similarGroupCount} 组相似 · 查看
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearSimilar} title="清除相似检测结果">
            ✕
          </button>
        </>
      )}
      {/* 动图检测：命中即高亮，勾选后走「删除所选」批量进回收站 */}
      {!animatedDetected && !detectingAnimated && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectAnimated}
          title="按文件头数帧，找出 GIF/APNG/动态 WebP/AVIF 序列等多帧动图，高亮后可批量移入回收站"
        >
          检测动图
        </button>
      )}
      {detectingAnimated && (
        <button type="button" className="toolbar-chip" disabled title="正在逐个读取图片头部数帧">
          检测动图…
        </button>
      )}
      {animatedDetected && (
        <>
          {animatedCount > 0 ? (
            <button
              type="button"
              className="toolbar-chip"
              onClick={onSelectAnimated}
              title="进入多选并勾中当前列表里的动图，用「删除所选」批量移入回收站"
            >
              {animatedCount} 张动图 · 勾选
            </button>
          ) : (
            <span className="toolbar-chip" title="库里已没有多帧动图">
              未检出动图
            </span>
          )}
          <button type="button" className="toolbar-chip" onClick={onClearAnimated} title="清除动图检测结果">
            ✕
          </button>
        </>
      )}
      <button
        type="button"
        className="toolbar-chip"
        onClick={onGenerateThumbnails}
        disabled={generating || withoutThumbnailCount === 0}
        title={withoutThumbnailCount === 0 ? "所有图片都已有缩略图" : "用 ffmpeg 等比缩放导出小图，浏览时不必解码原图"}
      >
        {generating
          ? `生成缩略图 ${thumbProgress ? `${thumbProgress.processed}/${thumbProgress.total}` : ""}`
          : withoutThumbnailCount === 0
            ? "缩略图已就绪"
            : `生成缩略图 (${withoutThumbnailCount})`}
      </button>
      {!selectMode ? (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onEnterSelect}
          title="进入多选模式：点选单张，Shift 点选连一段，Del 删除所选"
        >
          多选
        </button>
      ) : (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onToggleSelectAll}
            disabled={deletingSelected}
            title="全选/取消全选当前列表"
          >
            {allSelected ? "取消全选" : "全选"}
          </button>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onDeleteSelected}
            disabled={deletingSelected || movingSelected || selectedCount === 0}
            title="将选中的图片移入回收站（Del）"
          >
            {deletingSelected ? "删除中…" : `删除所选 (${selectedCount})`}
          </button>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onMoveSelected}
            disabled={deletingSelected || movingSelected || selectedCount === 0}
            title="将选中的图片移动到指定文件夹"
          >
            {movingSelected ? "移动中…" : "移动所选"}
          </button>
          <button type="button" className="toolbar-chip" onClick={onExitSelect} disabled={deletingSelected || movingSelected} title="退出多选模式">
            退出
          </button>
        </>
      )}
    </div>
  );
}
