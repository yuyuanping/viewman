import type { SortField, SortDirection, WatchState } from "../libraryFilter";
import { hevcNativeSupported } from "../utils";

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
  fakeCount: number;
  onConvertFakes: () => void;
  onClearFakes: () => void;
  convertingFakes: boolean;
  shortCount: number;
  onConvertShorts: () => void;
  convertingShorts: boolean;
  shortsDetected: boolean;
  onDetectShorts: () => void;
  detectingShorts: boolean;
  onClearShorts: () => void;
  onGenerateThumbnails: () => void;
  generating: boolean;
  thumbProgress: { processed: number; total: number } | null;
  withoutThumbnailCount: number;
  onDetectHevc: () => void;
  detectingHevc: boolean;
  hevcDetectProgress: { processed: number; total: number } | null;
  hevcDetected: boolean;
  hevcCount: number;
  onConvertHevc: () => void;
  convertingHevc: boolean;
  hevcProgress: { processed: number; total: number } | null;
  onClearHevc: () => void;
  onDetectDuplicates: () => void;
  detectingDuplicates: boolean;
  duplicatesDetected: boolean;
  duplicateGroupCount: number;
  duplicateExtrasCount: number;
  onDeleteDuplicates: () => void;
  deletingDuplicates: boolean;
  onClearDuplicates: () => void;
  selectMode: boolean;
  selectedCount: number;
  allSelected: boolean;
  onEnterSelect: () => void;
  onToggleSelectAll: () => void;
  onDeleteSelected: () => void;
  deletingSelected: boolean;
  onExitSelect: () => void;
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
  fakeCount,
  onConvertFakes,
  onClearFakes,
  convertingFakes,
  shortCount,
  onConvertShorts,
  convertingShorts,
  shortsDetected,
  onDetectShorts,
  detectingShorts,
  onClearShorts,
  onGenerateThumbnails,
  generating,
  thumbProgress,
  withoutThumbnailCount,
  onDetectHevc,
  detectingHevc,
  hevcDetectProgress,
  hevcDetected,
  hevcCount,
  onConvertHevc,
  convertingHevc,
  hevcProgress,
  onClearHevc,
  onDetectDuplicates,
  detectingDuplicates,
  duplicatesDetected,
  duplicateGroupCount,
  duplicateExtrasCount,
  onDeleteDuplicates,
  deletingDuplicates,
  onClearDuplicates,
  selectMode,
  selectedCount,
  allSelected,
  onEnterSelect,
  onToggleSelectAll,
  onDeleteSelected,
  deletingSelected,
  onExitSelect,
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
      {fakeCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onConvertFakes}
            disabled={convertingFakes}
            title="按真实内容另存为 .jpg/.png 等图片文件，源文件移入回收站并移出视频库"
          >
            {convertingFakes ? "转换中…" : `${fakeCount} 个假视频（图片） · 转换为图片`}
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearFakes} disabled={convertingFakes} title="清除假视频标记">
            ✕
          </button>
        </>
      )}
      {!shortsDetected && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectShorts}
          disabled={detectingShorts || convertingShorts}
          title="扫描时长 ≤5 秒且画面近似静图/幻灯片的视频，找出可转图片的候选"
        >
          {detectingShorts ? "检测短视频中…" : "检测静图短视频"}
        </button>
      )}
      {shortsDetected && shortCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onConvertShorts}
            disabled={convertingShorts}
            title="抽帧生成同名 .jpg，原视频移入回收站并移出视频库"
          >
            {convertingShorts ? "转换中…" : `${shortCount} 个静图短视频 · 抽帧转图片`}
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearShorts} disabled={convertingShorts} title="清除检测结果">
            ✕
          </button>
        </>
      )}
      {!duplicatesDetected && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectDuplicates}
          disabled={detectingDuplicates || deletingDuplicates}
          title="按文件大小 + 内容指纹分组比对，找出内容完全相同的重复视频"
        >
          {detectingDuplicates ? "比对中…" : "检测重复视频"}
        </button>
      )}
      {duplicatesDetected && duplicateGroupCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onDeleteDuplicates}
            disabled={deletingDuplicates}
            title="每组保留最早添加的一个，将其余副本移入回收站"
          >
            {deletingDuplicates ? "删除中…" : `${duplicateGroupCount} 组重复 · 删除 ${duplicateExtrasCount} 个副本`}
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearDuplicates} disabled={deletingDuplicates} title="清除检测结果">
            ✕
          </button>
        </>
      )}
      {!hevcNativeSupported && !hevcDetected && (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDetectHevc}
          disabled={detectingHevc || convertingHevc}
          title="扫描 HEVC 编码的视频，可一次性永久转码为 H.264（原文件进回收站）"
        >
          {detectingHevc
            ? `检测编码中 ${hevcDetectProgress ? `${hevcDetectProgress.processed}/${hevcDetectProgress.total}` : "…"}`
            : "检测 HEVC"}
        </button>
      )}
      {!hevcNativeSupported && hevcDetected && hevcCount > 0 && (
        <>
          <button
            type="button"
            className="toolbar-chip"
            onClick={onConvertHevc}
            disabled={convertingHevc}
            title="就地重编码为 H.264 + AAC，原文件移入回收站；此后无需每次播放再转码"
          >
            {convertingHevc
              ? `转码中 ${hevcProgress ? `${hevcProgress.processed}/${hevcProgress.total}` : ""}`
              : `${hevcCount} 个 HEVC · 永久转码 H.264`}
          </button>
          <button type="button" className="toolbar-chip" onClick={onClearHevc} disabled={convertingHevc} title="清除检测结果">
            ✕
          </button>
        </>
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
      {!selectMode ? (
        <button
          type="button"
          className="toolbar-chip"
          onClick={onEnterSelect}
          title="进入多选模式，点选多个视频后批量删除"
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
            disabled={deletingSelected || selectedCount === 0}
            title="将选中的视频移入回收站"
          >
            {deletingSelected ? "删除中…" : `删除所选 (${selectedCount})`}
          </button>
          <button type="button" className="toolbar-chip" onClick={onExitSelect} disabled={deletingSelected} title="退出多选模式">
            退出
          </button>
        </>
      )}
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
