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
  duplicatesDetected: boolean;
  duplicateGroupCount: number;
  duplicateExtrasCount: number;
  onDeleteDuplicates: () => void;
  deletingDuplicates: boolean;
  onClearDuplicates: () => void;
  /** 相似图检测（pHash）：只高亮不删除 */
  onDetectSimilar: () => void;
  detectingSimilar: boolean;
  similarDetected: boolean;
  similarGroupCount: number;
  onClearSimilar: () => void;
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
  duplicatesDetected,
  duplicateGroupCount,
  duplicateExtrasCount,
  onDeleteDuplicates,
  deletingDuplicates,
  onClearDuplicates,
  onDetectSimilar,
  detectingSimilar,
  similarDetected,
  similarGroupCount,
  onClearSimilar,
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

      {!duplicatesDetected && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectDuplicates}
          disabled={detectingDuplicates || deletingDuplicates}
          title="按文件大小 + 内容指纹分组比对，找出内容完全相同的重复图片"
        >
          {detectingDuplicates ? "比对中…" : "检测重复图片"}
        </button>
      )}
      {!similarDetected && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectSimilar}
          disabled={detectingSimilar}
          title="感知哈希比对，找出相似但不相同的连拍/截图系列（只高亮不删除）"
        >
          {detectingSimilar ? "比对中…" : "检测相似图片"}
        </button>
      )}
      {similarDetected && similarGroupCount > 0 && (
        <>
          <span className="toolbar-label">{similarGroupCount} 组相似</span>
          <button type="button" className="toolbar-chip" onClick={onClearSimilar} title="清除相似检测结果">
            ✕
          </button>
        </>
      )}
      {duplicatesDetected && duplicateGroupCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onDeleteDuplicates}
            disabled={deletingDuplicates}
            title="每组保留最早添加的一张，将其余副本移入回收站"
          >
            {deletingDuplicates ? "删除中…" : `${duplicateGroupCount} 组重复 · 删除 ${duplicateExtrasCount} 个副本`}
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearDuplicates} disabled={deletingDuplicates} title="清除检测结果">
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
          title="进入多选模式，点选多张图片后批量删除"
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
            title="将选中的图片移入回收站"
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
