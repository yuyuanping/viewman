import { useEffect, useMemo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Image } from "../types";
import type { SimilarGroup } from "../similarGroups";
import { formatFileSize, formatResolution } from "../utils";

interface SimilarGroupsPanelProps {
  /** 相似组：已裁掉不在库的组员，keep 是本组保留的那张 */
  groups: SimilarGroup[];
  imageById: Map<string, Image>;
  selectedIds: Set<string>;
  onToggle: (image: Image) => void;
  /** 把该组的保留项换成这张，其余重新勾上 */
  onKeep: (groupAt: number, keepId: string) => void;
  /** 按"每组只留第一张"重刷勾选 */
  onAutoSelect: () => void;
  /** 当前列表里已勾中的条目数（含相似组之外的） */
  selectedTotal: number;
  /** 直接走工具栏那条批量删除通路（进回收站前有二次确认） */
  onDeleteSelected: () => void;
  deleting: boolean;
  /** 放大看：以整组为翻页范围，方便逐张比对 */
  onOpenImage: (image: Image, groupIds: string[]) => void;
  onClose: () => void;
}

/**
 * 相似图分组审阅面板：每组单独一块，组员并排显示，勾中的走"删除所选"那条通路。
 * 块上加了 content-visibility，几千组时只有滚进视口的才参与布局。
 */
export function SimilarGroupsPanel({
  groups, imageById, selectedIds, onToggle, onKeep, onAutoSelect, selectedTotal, onDeleteSelected, deleting, onOpenImage, onClose,
}: SimilarGroupsPanelProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const selectedInGroups = useMemo(
    () => groups.reduce((n, group) => n + group.ids.filter(id => selectedIds.has(id)).length, 0),
    [groups, selectedIds],
  );

  return (
    <div className="image-viewer fixed inset-0 z-40 flex flex-col" role="dialog" aria-modal="true" aria-label="相似图片分组">
      <div className="flex items-center gap-3 px-4 py-3 shrink-0">
        <h2 className="text-sm text-gray-200 font-medium">相似图片分组</h2>
        <span className="text-xs text-gray-400 tabular-nums">{groups.length} 组 · 组内已勾 {selectedInGroups}</span>
        <span className="toolbar-spacer" />
        <button
          type="button"
          className="toolbar-chip"
          onClick={onAutoSelect}
          title="每组只留当前标着「保留」的那张，其余全部勾上"
        >
          重选副本
        </button>
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDeleteSelected}
          disabled={deleting || selectedTotal === 0}
          title="把已勾中的图片移入回收站（Del，删除前会再确认一次）"
        >
          {deleting ? "删除中…" : `删除所选 (${selectedTotal})`}
        </button>
        <button type="button" className="toolbar-chip" onClick={onClose} title="关闭（Esc）" aria-label="关闭相似分组面板">
          ✕
        </button>
      </div>
      <p className="px-4 pb-2 text-xs text-gray-500 shrink-0">
        点缩略图勾选/取消，点「留」把本组的保留项换成这张；按 Del 一次性移入回收站。
      </p>

      <div className="flex-1 overflow-y-auto px-4 pb-6">
        {groups.length === 0 && (
          <p className="text-gray-500 text-sm p-4">没有可审阅的相似组了（组员少于两张就不算一组）。</p>
        )}
        {groups.map((group, index) => {
          const images = group.ids.map(id => imageById.get(id)).filter((image): image is Image => image !== undefined);
          const selectedInGroup = group.ids.filter(id => selectedIds.has(id)).length;
          return (
            <section
              key={group.at}
              className="mb-5"
              style={{ contentVisibility: "auto", containIntrinsicSize: "auto 190px" }}
            >
              <header className="flex items-center gap-2 text-xs text-gray-400 mb-2 sticky top-0 py-1 bg-[#04070de6] z-10">
                <span className="text-gray-200 font-medium">组 {index + 1}</span>
                <span className="tabular-nums">{images.length} 张</span>
                <span className={`tabular-nums ${selectedInGroup > 0 ? "text-blue-400" : "text-gray-500"}`}>
                  已勾 {selectedInGroup}
                </span>
                <span className="text-gray-600 truncate">{imageById.get(group.keep)?.filename}</span>
              </header>
              <ul className="flex flex-wrap gap-2.5">
                {images.map(image => {
                  const selected = selectedIds.has(image.id);
                  const isKeep = group.keep === image.id;
                  return (
                    <li key={image.id} className="relative w-[128px] min-w-0">
                      <div className={`rounded-lg overflow-hidden border border-white/10 bg-[#151f2e]${selected ? " media-selected" : ""}`}>
                        <button
                          type="button"
                          onClick={() => onToggle(image)}
                          aria-pressed={selected}
                          className="block w-full text-left"
                          title={selected ? `取消勾选 ${image.filename}` : `勾选 ${image.filename}`}
                        >
                          <span className="relative block aspect-square bg-[#0f1723] overflow-hidden">
                            <img
                              src={convertFileSrc(image.thumbnail_path ?? image.path)}
                              alt={image.filename}
                              loading="lazy"
                              className="w-full h-full object-cover"
                            />
                            {selected && (
                              <span className="absolute top-1 left-1 w-5 h-5 grid place-items-center rounded-full bg-blue-600 text-white text-[12px] font-bold" aria-hidden="true">✓</span>
                            )}
                            {isKeep && (
                              <span className="absolute bottom-1 left-1 px-1.5 py-0.5 rounded bg-gray-950/85 text-[10px] text-emerald-300">保留</span>
                            )}
                          </span>
                          <span className="block px-1.5 pt-1.5 text-[11px] leading-4 text-gray-200 truncate" title={image.filename}>
                            {image.filename}
                          </span>
                          <span className="block px-1.5 pb-1.5 text-[10px] text-gray-500 tabular-nums">
                            {formatFileSize(image.file_size)} · {formatResolution(image.width, image.height)}
                          </span>
                        </button>
                      </div>
                      <span className="absolute top-1.5 right-1.5 flex gap-1">
                        <button
                          type="button"
                          onClick={() => onOpenImage(image, group.ids)}
                          className="w-6 h-6 grid place-items-center rounded bg-gray-950/80 hover:bg-gray-700 text-[11px] text-gray-200"
                          title="看大图"
                          aria-label={`放大 ${image.filename}`}
                        >
                          ⤢
                        </button>
                        <button
                          type="button"
                          onClick={() => onKeep(group.at, image.id)}
                          disabled={isKeep}
                          className="px-1.5 h-6 grid place-items-center rounded bg-gray-950/80 hover:bg-blue-700 disabled:opacity-40 text-[10px] text-gray-200"
                          title="本组改为保留这张，其余勾上"
                          aria-label={`保留 ${image.filename}`}
                        >
                          留
                        </button>
                      </span>
                    </li>
                  );
                })}
              </ul>
            </section>
          );
        })}
      </div>
    </div>
  );
}
