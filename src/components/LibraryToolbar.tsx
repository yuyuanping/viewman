import type { SortField, SortDirection, WatchState } from "../libraryFilter";

interface LibraryToolbarProps {
  sortField: SortField;
  sortDirection: SortDirection;
  onSortFieldChange: (field: SortField) => void;
  onToggleDirection: () => void;
  watchState: WatchState;
  onWatchStateChange: (state: WatchState) => void;
  onCheckFiles: () => void;
  checking: boolean;
  checkProgress: { processed: number; total: number } | null;
  missingCount: number;
  onClearMissing: () => void;
  onGenerateThumbnails: () => void;
  generating: boolean;
  thumbProgress: { processed: number; total: number } | null;
  withoutThumbnailCount: number;
}

const SORT_OPTIONS: { value: SortField; label: string }[] = [
  { value: "filename", label: "文件名" },
  { value: "duration", label: "时长" },
  { value: "file_size", label: "文件大小" },
  { value: "created_at", label: "添加时间" },
];

const WATCH_OPTIONS: { value: WatchState; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "unwatched", label: "未看" },
  { value: "in_progress", label: "在看" },
  { value: "finished", label: "已看完" },
];

export function LibraryToolbar({
  sortField,
  sortDirection,
  onSortFieldChange,
  onToggleDirection,
  watchState,
  onWatchStateChange,
  onCheckFiles,
  checking,
  checkProgress,
  missingCount,
  onClearMissing,
  onGenerateThumbnails,
  generating,
  thumbProgress,
  withoutThumbnailCount,
}: LibraryToolbarProps) {
  return (
    <div className="library-toolbar shrink-0">
      <span className="toolbar-label">排序</span>
      <select
        className="toolbar-select"
        value={sortField}
        onChange={(e) => onSortFieldChange(e.target.value as SortField)}
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

      <span className="toolbar-label" style={{ marginLeft: 8 }}>观看状态</span>
      {WATCH_OPTIONS.map(option => (
        <button
          key={option.value}
          type="button"
          className="toolbar-chip"
          aria-pressed={watchState === option.value}
          onClick={() => onWatchStateChange(option.value)}
        >
          {option.label}
        </button>
      ))}

      <span className="toolbar-spacer" />

      {missingCount > 0 && (
        <button type="button" className="toolbar-chip" onClick={onClearMissing} title="清除丢失标记">
          {missingCount} 个文件丢失 · 清除
        </button>
      )}
      <button
        type="button"
        className="toolbar-chip"
        onClick={onGenerateThumbnails}
        disabled={generating || withoutThumbnailCount === 0}
        title={withoutThumbnailCount === 0 ? "所有视频都已有封面" : "使用 ffmpeg 抽取画面作为封面"}
      >
        {generating
          ? `生成封面 ${thumbProgress ? `${thumbProgress.processed}/${thumbProgress.total}` : ""}`
          : withoutThumbnailCount === 0
            ? "封面已就绪"
            : `生成封面 (${withoutThumbnailCount})`}
      </button>
      <button
        type="button"
        className="toolbar-chip"
        onClick={onCheckFiles}
        disabled={checking}
        title="逐个检查文件是否仍可读取"
      >
        {checking
          ? `检查中 ${checkProgress ? `${checkProgress.processed}/${checkProgress.total}` : ""}`
          : "检查文件状态"}
      </button>
    </div>
  );
}
