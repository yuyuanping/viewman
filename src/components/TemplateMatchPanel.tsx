import { useEffect, useMemo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Image } from "../types";
import type { TemplateMatch } from "../api";
import { formatFileSize, formatResolution } from "../utils";
import { JustifiedGrid } from "./JustifiedGrid";
import { TEMPLATE_THRESHOLD_MAX } from "../hooks/useTemplateSearch";

/** JustifiedGrid 的稳定回调：内联箭头函数每次渲染都换引用，会把布局缓存全部打穿 */
const aspectOfEntry = (entry: { image: Image }) =>
  entry.image.width && entry.image.height ? entry.image.width / entry.image.height : 1;
const keyOfEntry = (entry: { image: Image }) => entry.image.id;

interface TemplateMatchPanelProps {
  /** 当模板的那张图：面板头部要显示"在拿谁比" */
  template: Image | null;
  /** 全部命中（已按距离升序），阈值过滤在这里做 */
  matches: TemplateMatch[];
  imageById: Map<string, Image>;
  /** 解不出指纹、没参与比对的张数 */
  skipped: number;
  threshold: number;
  onThreshold: (threshold: number) => void;
  searching: boolean;
  progress: { processed: number; total: number } | null;
  selectedIds: Set<string>;
  onToggle: (image: Image) => void;
  /** 勾选翻转给定 id 清单：on=true 全勾上，on=false 全取消（"全选命中"按钮用） */
  onSelectIds: (ids: string[], on: boolean) => void;
  /** 清空整个待删清单（含面板之外的勾选） */
  onClearAll: () => void;
  selectedTotal: number;
  onDeleteSelected: () => void;
  deleting: boolean;
  onMoveSelected?: () => void;
  moving?: boolean;
  /** 放大看：以命中清单为翻页范围 */
  onOpenImage: (image: Image, matchIds: string[]) => void;
  onClose: () => void;
}

/**
 * 模板匹配结果面板：一份按距离升序的命中清单，与相似检测的分组面板不同——
 * 这里没有"组"和保留张，就是"谁跟模板像、像到几分"，勾删由人逐张决定。
 * 宽容度滑杆只在前端过滤已返回的命中，改了不用重跑。
 */
export function TemplateMatchPanel({
  template, matches, imageById, skipped, threshold, onThreshold, searching, progress,
  selectedIds, onToggle, onSelectIds, onClearAll, selectedTotal, onDeleteSelected, deleting,
  onMoveSelected, moving, onOpenImage, onClose,
}: TemplateMatchPanelProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const visible = useMemo(
    () => matches
      .filter(hit => hit.distance <= threshold)
      .map(hit => ({ hit, image: imageById.get(hit.id) }))
      .filter((row): row is { hit: TemplateMatch; image: Image } => row.image !== undefined),
    [matches, threshold, imageById],
  );
  const visibleIds = useMemo(() => visible.map(row => row.image.id), [visible]);
  const selectedInList = visibleIds.filter(id => selectedIds.has(id)).length;
  const allChecked = visibleIds.length > 0 && selectedInList === visibleIds.length;

  return (
    <div className="image-viewer fixed inset-0 z-40 flex flex-col" role="dialog" aria-modal="true" aria-label="模板匹配结果">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3 shrink-0">
        <h2 className="text-sm text-gray-200 font-medium">模板匹配</h2>
        {template && (
          <span className="flex items-center gap-2 text-xs text-gray-400 min-w-0">
            <img
              src={convertFileSrc(template.thumbnail_path ?? template.path)}
              alt=""
              className="w-7 h-7 rounded object-contain bg-[#0f1723] shrink-0"
            />
            <span className="truncate max-w-48" title={template.path}>{template.filename}</span>
          </span>
        )}
        <span className="text-xs text-gray-400 tabular-nums">
          {searching
            ? `比对中${progress ? ` · 补算指纹 ${progress.processed}/${progress.total}` : "…"}`
            : `命中 ${visible.length}/${matches.length} 张 · 已勾 ${selectedInList}`}
          {skipped > 0 && ` · ${skipped} 张解不出指纹，未参与`}
        </span>
        <label
          className="flex items-center gap-1.5 text-xs text-gray-400"
          title="汉明距离阈值（两枚指纹的距离之和）：只显示与模板差这么多位以内的图。往右更宽松，找得多、撞车也多。命中清单已整份返回，改这里只是前端过滤"
        >
          宽容度
          <input
            type="range"
            min={0}
            max={TEMPLATE_THRESHOLD_MAX}
            step={1}
            value={threshold}
            onChange={(e) => onThreshold(Number(e.target.value))}
            className="w-28 accent-blue-500"
            aria-label="模板匹配宽容度（汉明距离阈值）"
          />
          <span className="tabular-nums text-gray-200">{threshold}</span>
        </label>
        <span className="toolbar-spacer" />
        <button
          type="button"
          className="toolbar-chip"
          disabled={visibleIds.length === 0}
          onClick={() => onSelectIds(visibleIds, !allChecked)}
          title={allChecked ? "取消本清单的勾选" : "当前阈值内的命中全部勾上"}
        >
          {allChecked ? "取消全选" : "全选命中"}
        </button>
        <button
          type="button"
          className="toolbar-chip"
          onClick={onClearAll}
          disabled={selectedTotal === 0}
          title="取消所有勾选（含本面板之外的），这样「删除所选」不会删掉任何东西"
        >
          取消全选
        </button>
        {onMoveSelected && (
          <button
            type="button"
            className="toolbar-chip"
            onClick={onMoveSelected}
            disabled={moving || selectedTotal === 0}
            title="把已勾中的图片移动到另一个文件夹（M，会先弹目录选择器，移动前再确认一次）"
          >
            {moving ? "移动中…" : `移动所选 (${selectedTotal})`}
          </button>
        )}
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDeleteSelected}
          disabled={deleting || selectedTotal === 0}
          title="把已勾中的图片移入回收站（Del，删除前会再确认一次）"
        >
          {deleting ? "删除中…" : `删除所选 (${selectedTotal})`}
        </button>
        <button type="button" className="toolbar-chip" onClick={onClose} title="关闭（Esc）" aria-label="关闭模板匹配面板">
          关闭 (Esc)
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-4 pb-6">
        {searching && matches.length === 0 && (
          <p className="text-gray-500 text-sm p-4">
            正在比对全库指纹{progress ? `（补算指纹 ${progress.processed}/${progress.total}）` : ""}…
          </p>
        )}
        {!searching && visible.length === 0 && (
          <p className="text-gray-500 text-sm p-4">
            {matches.length === 0
              ? "库里没有与模板相近的图片。"
              : `距离 ≤ ${threshold} 的命中为零——把宽容度往右拖试试。`}
          </p>
        )}
        {visible.length > 0 && (
          <JustifiedGrid
            items={visible}
            aspectOf={aspectOfEntry}
            keyOf={keyOfEntry}
            textH={46}
            targetImageH={200}
            renderItem={({ hit, image }, imageH) => {
              const selected = selectedIds.has(image.id);
              return (
                <div className="relative min-w-0">
                  <div className={`rounded-lg overflow-hidden border border-white/10 bg-[#151f2e]${selected ? " media-selected" : ""}`}>
                    <button
                      type="button"
                      onClick={() => onToggle(image)}
                      aria-pressed={selected}
                      className="block w-full text-left"
                      title={selected ? `取消勾选 ${image.filename}` : `勾选 ${image.filename}`}
                    >
                      <span className="relative block bg-[#0f1723] overflow-hidden" style={{ height: imageH }}>
                        <img
                          src={convertFileSrc(image.thumbnail_path ?? image.path)}
                          alt={image.filename}
                          loading="lazy"
                          className="w-full h-full object-contain"
                        />
                        {selected && (
                          <span className="absolute top-1 left-1 w-5 h-5 grid place-items-center rounded-full bg-blue-600 text-white text-[12px] font-bold" aria-hidden="true">✓</span>
                        )}
                      </span>
                      <span className="block px-1.5 pt-1.5 pb-1.5 overflow-hidden" style={{ height: 46 }}>
                        <span className="block text-[11px] leading-4 text-gray-200 truncate" title={image.filename}>
                          {image.filename}
                        </span>
                        <span className="block text-[10px] leading-4 text-gray-500 mt-0.5 tabular-nums">
                          {formatFileSize(image.file_size)} · {formatResolution(image.width, image.height)}
                          {` · `}
                          <span className={hit.distance === 0 ? "text-emerald-400" : hit.distance <= 24 ? "text-sky-300" : undefined}>
                            距 {hit.distance}
                          </span>
                        </span>
                      </span>
                    </button>
                  </div>
                  <span className="absolute top-1.5 right-1.5">
                    <button
                      type="button"
                      onClick={() => onOpenImage(image, visibleIds)}
                      className="w-6 h-6 grid place-items-center rounded bg-gray-950/80 hover:bg-gray-700 text-[11px] text-gray-200"
                      title="看大图"
                      aria-label={`放大 ${image.filename}`}
                    >
                      ⤢
                    </button>
                  </span>
                </div>
              );
            }}
          />
        )}
      </div>
    </div>
  );
}
