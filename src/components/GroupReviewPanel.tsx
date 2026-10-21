import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Image } from "../types";
import type { SimilarGroup } from "../similarGroups";
import { groupsMatching } from "../similarGroups";
import { formatFileSize, formatResolution } from "../utils";
import { JustifiedGrid } from "./JustifiedGrid";
import { ContextMenu, useContextMenu, useFileMenuActions } from "./ContextMenu";

/** 一组超过这个张数就先折起来：几百张一次铺出来既卡又没法逐张比对 */
const COLLAPSE_AT = 24;

/** JustifiedGrid 的稳定回调：内联箭头函数每次渲染都换引用，会把布局缓存全部打穿 */
const aspectOfEntry = (entry: { image: Image }) =>
  entry.image.width && entry.image.height ? entry.image.width / entry.image.height : 1;
const keyOfEntry = (entry: { image: Image }) => entry.image.id;

interface GroupReviewPanelProps {
  /** 文案里指代这一批条目的名词（"重复图片"），标题和提示都由它拼 */
  noun: string;
  /** 进度前缀：重复检测是"核对候选" */
  progressLabel: string;
  /** 组：已裁掉不在库的组员，keep 是本组保留的那张 */
  groups: SimilarGroup[];
  imageById: Map<string, Image>;
  selectedIds: Set<string>;
  onToggle: (image: Image) => void;
  /** 把该组的保留项换成这张，其余重新勾上（第一参数是本组当前的保留张） */
  onKeep: (fromKeep: string, keepId: string) => void;
  /** 按"每组只留当前保留张"重刷勾选 */
  onAutoSelect: () => void;
  /** 本面板全部组员（含保留张）一次勾上 */
  onSelectAll: () => void;
  /** 清空整个待删清单（含本面板之外的勾选） */
  onClearAll: () => void;
  /** 整组勾选翻转：on=true 连保留张一起勾，on=false 只取消这组的 */
  onSelectGroup: (ids: string[], on: boolean) => void;
  /** 当前列表里已勾中的条目数（含本面板之外的） */
  selectedTotal: number;
  /** 直接走工具栏那条批量删除通路 */
  onDeleteSelected: () => void;
  deleting: boolean;
  /** 批量移动勾中的图片到别的文件夹（弹系统目录选择器）；不给就不显示这个按钮 */
  onMoveSelected?: () => void;
  moving?: boolean;
  /** 扩展名修正（重复检测给：解不出画面的多半是内容与扩展名不符）；不给就不显示这个按钮 */
  onFixExtensions?: () => void;
  fixingExtensions?: boolean;
  /** 检测仍在跑：每推一次，分组整份换新 */
  detecting: boolean;
  /** 补算/核对的进度；没有中间进度时这条不出现 */
  progress: { processed: number; total: number } | null;
  /** 结论的保留意见（比如"另有 N 张解不出画面"）：不说清的话"没有重复"会被当成定论 */
  caveat?: string;
  /** 是否比对像素：关掉就是快检（只看双指纹门槛，误判多、秒回）。不给就不显示这个开关 */
  pixelVerify?: boolean;
  onPixelVerify?: (verify: boolean) => void;
  /** 放大看：以整组为翻页范围，方便逐张比对 */
  onOpenImage: (image: Image, groupIds: string[]) => void;
  /** 单张移动：右键菜单用，接到父层的移动对话框 */
  onMoveImage?: (image: Image) => void;
  /** 单张删除：右键菜单用，进回收站并清理勾选 */
  onDeleteImage?: (image: Image) => void;
  /** 非阻塞错误回报（父层转 toast） */
  onError?: (message: string) => void;
  /** 大图查看器是否盖在本面板上（父层知道 viewerIndex）：为真时 Esc 只关大图，不关本面板。
   *  两者都在 window 上挂 Esc 监听，一次按键两个都会响应 */
  viewerOpen?: boolean;
  onClose: () => void;
}

/**
 * 分组审阅面板（重复图）：每组单独一块，组员并排显示，勾中的走"删除所选"那条通路。
 * 检测是边算边推的，所以面板开着就能看到逐渐变长的清单，不必等全库跑完。
 * 块上加了 content-visibility，几千组时只有滚进视口的才参与布局。
 * 比对像素开关只有重复检测给。
 */
export function GroupReviewPanel({
  noun, progressLabel, groups, imageById, selectedIds, onToggle, onKeep, onAutoSelect, onSelectAll, onClearAll, onSelectGroup, selectedTotal, onDeleteSelected, deleting, onMoveSelected, moving, onFixExtensions, fixingExtensions, detecting, progress, caveat, pixelVerify, onPixelVerify, onOpenImage, onMoveImage, onDeleteImage, onError, viewerOpen, onClose,
}: GroupReviewPanelProps) {
  /** 展开过的组（按当前的保留张记着）：组员太多的组默认只铺前若干张 */
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  /** 组名/路径筛选：几万组里要能直接查到某一枚，而不是滚到眼花 */
  const [query, setQuery] = useState("");
  // 面板级只量一次内容宽，共享给所有分组：分组数上千时每组各挂
  // ResizeObserver + 挂载二次渲染，打开面板就等于卡死
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [contentWidth, setContentWidth] = useState(0);

  useEffect(() => {
    // 大图在最上层：Esc 归它（它自己会关掉大图），本面板这一层别跟着收
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && !viewerOpen) onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, viewerOpen]);

  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;
    const sync = () => {
      const cs = getComputedStyle(el);
      setContentWidth(el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight));
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const selectedInGroups = useMemo(
    () => groups.reduce((n, group) => n + group.ids.filter(id => selectedIds.has(id)).length, 0),
    [groups, selectedIds],
  );

  const visible = useMemo(() => groupsMatching(groups, query, imageById), [groups, query, imageById]);

  // 组员右键菜单：与网格卡片同一套（打开所在位置 / 复制文件名 / 移动到… / 删除到回收站）
  const { menu, openMenu, closeMenu } = useContextMenu<Image>();
  const { reveal, copyName } = useFileMenuActions(onError);

  return (
    <div className="image-viewer fixed inset-0 z-40 flex flex-col" role="dialog" aria-modal="true" aria-label={`${noun}分组`}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3 shrink-0">
        <h2 className="text-sm text-gray-200 font-medium">{`${noun}分组`}</h2>
        <span className="text-xs text-gray-400 tabular-nums">
          {groups.length} 组 · 组内已勾 {selectedInGroups}
          {progress && ` · ${progressLabel} ${progress.processed}/${progress.total}`}
          {caveat && ` · ${caveat}`}
        </span>
        {onPixelVerify && pixelVerify !== undefined && (
          <label
            className="flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer"
            title="开：候选组逐张解像素核对（准，慢，首次几分钟）；关：只看双指纹门槛直接成组（快，误判多，靠人工复核）。改完重跑一次检测生效"
          >
            <input
              type="checkbox"
              checked={pixelVerify}
              disabled={detecting}
              onChange={(e) => onPixelVerify(e.target.checked)}
              className="accent-blue-500"
              aria-label="是否比对像素"
            />
            比对像素
          </label>
        )}
        <span className="toolbar-spacer" />
        <button
          type="button"
          className="toolbar-chip"
          onClick={onSelectAll}
          title="本面板所有组员（含每组标着「保留」的那张）全部勾上"
        >
          全选
        </button>
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
            title="把已勾中的图片移动到另一个文件夹（M，会先弹目录选择器）"
          >
            {moving ? "移动中…" : `移动所选 (${selectedTotal})`}
          </button>
        )}
        <button
          type="button"
          className="toolbar-chip"
          onClick={onDeleteSelected}
            disabled={deleting || selectedTotal === 0}
            title="把已勾中的图片移入回收站（Del）"
        >
          {deleting ? "删除中…" : `删除所选 (${selectedTotal})`}
        </button>
        {onFixExtensions && caveat && (
          <button
            type="button"
            className="toolbar-chip"
            onClick={onFixExtensions}
            disabled={fixingExtensions || detecting}
            title="按文件头把内容与扩展名不符的图就地改名并同步库记录；改完自动重跑检测，把这批图收进比对"
          >
            {fixingExtensions ? "修正中…" : "修正扩展名"}
          </button>
        )}
        <button type="button" className="toolbar-chip" onClick={onClose} title="关闭（Esc；大图开着时 Esc 先关大图）" aria-label={`关闭${noun}分组面板`}>
          关闭 (Esc)
        </button>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 pb-2 shrink-0">
        <input
          type="search"
          className="toolbar-input w-64 min-w-0"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="筛组：文件名或路径片段"
          aria-label={`按文件名或路径筛选${noun}组`}
        />
        <span className="text-xs text-gray-500">
          {query && `命中 ${visible.length}/${groups.length} 组 · `}
          {detecting
            ? `还在比对剩下的${noun}，分组会随进度整份刷新；已出的组现在就能勾、能删。`
            : "点缩略图勾选/取消，点「留」把本组的保留项换成这张；按 M 移动所选，按 Del 一次性移入回收站。"}
        </span>
      </div>

      <div className="flex-1 overflow-y-auto px-4 pb-6" ref={contentRef}>
        {groups.length === 0 && (
          <p className="text-gray-500 text-sm p-4">
            {detecting
              ? `正在逐张比对，还没有成组的${noun}${progress ? `（${progressLabel} ${progress.processed}/${progress.total}）` : ""}…`
              : `没有可审阅的${noun}组了（组员少于两张就不算一组）。`}
          </p>
        )}
        {groups.length > 0 && visible.length === 0 && (
          <p className="text-gray-500 text-sm p-4">{`没有文件名或路径含「${query.trim()}」的${noun}组——这张图不在任何一组里。`}</p>
        )}
        {visible.map((group, index) => {
          const ranked = group.ids
            .map(id => imageById.get(id))
            .filter((image): image is Image => image !== undefined)
            .map(image => ({ image }));
          const folded = ranked.length > COLLAPSE_AT && !expanded.has(group.keep);
          const shown = folded ? ranked.slice(0, COLLAPSE_AT) : ranked;
          const memberIds = ranked.map(r => r.image.id);
          const selectedInGroup = memberIds.filter(id => selectedIds.has(id)).length;
          const allChecked = selectedInGroup === memberIds.length;
          return (
            <section
              key={group.keep}
              className="mb-5"
              style={{ contentVisibility: "auto", containIntrinsicSize: "auto 190px" }}
            >
              <header className="flex items-center gap-2 text-xs text-gray-400 mb-2 sticky top-0 py-1 bg-[#04070de6] z-10">
                <span className="text-gray-200 font-medium">组 {index + 1}</span>
                <span className="tabular-nums">{ranked.length} 张</span>
                <span className={`tabular-nums ${selectedInGroup > 0 ? "text-blue-400" : "text-gray-500"}`}>
                  已勾 {selectedInGroup}
                </span>
                <button
                  type="button"
                  className="toolbar-chip shrink-0"
                  onClick={() => onSelectGroup(memberIds, !allChecked)}
                  title={allChecked ? "取消本组的勾选" : "本组全部勾上（含标着「保留」的那张），删除时整组一起进回收站"}
                >
                  {allChecked ? "取消全选" : "全选"}
                </button>
                <span className="text-gray-600 truncate min-w-0 flex-1">{imageById.get(group.keep)?.filename}</span>
              </header>
              <JustifiedGrid
                items={shown}
                aspectOf={aspectOfEntry}
                keyOf={keyOfEntry}
                width={contentWidth}
                textH={46}
                targetImageH={200}
                renderItem={({ image }, imageH) => {
                  const selected = selectedIds.has(image.id);
                  const isKeep = group.keep === image.id;
                  return (
                    <div className="relative min-w-0" onContextMenu={(e) => openMenu(e, image)}>
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
                            {isKeep && (
                              <span className="absolute bottom-1 left-1 px-1.5 py-0.5 rounded bg-gray-950/85 text-[10px] text-emerald-300">保留</span>
                            )}
                          </span>
                          <span className="block px-1.5 pt-1.5 pb-1.5 overflow-hidden" style={{ height: 46 }}>
                            <span className="block text-[11px] leading-4 text-gray-200 truncate" title={image.filename}>
                              {image.filename}
                            </span>
                            <span className="block text-[10px] leading-4 text-gray-500 mt-0.5 tabular-nums">
                              {formatFileSize(image.file_size)} · {formatResolution(image.width, image.height)}
                            </span>
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
                          onClick={() => onKeep(group.keep, image.id)}
                          disabled={isKeep}
                          className="px-1.5 h-6 grid place-items-center rounded bg-gray-950/80 hover:bg-blue-700 disabled:opacity-40 text-[10px] text-gray-200"
                          title="本组改为保留这张，其余勾上"
                          aria-label={`保留 ${image.filename}`}
                        >
                          留
                        </button>
                      </span>
                    </div>
                  );
                }}
              />
              {ranked.length > COLLAPSE_AT && (
                <button
                  type="button"
                  className="toolbar-chip mt-2"
                  onClick={() => setExpanded(prev => {
                    const next = new Set(prev);
                    if (next.has(group.keep)) next.delete(group.keep); else next.add(group.keep);
                    return next;
                  })}
                  title="折起来的组员仍按当前勾选走，删除时一样进回收站"
                >
                  {folded ? `展开其余 ${ranked.length - COLLAPSE_AT} 张` : "收起"}
                </button>
              )}
            </section>
          );
        })}
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={closeMenu}
          label={`${menu.target.filename} 的操作菜单`}
          items={[
            { key: "reveal", label: "打开文件所在位置", onSelect: () => reveal(menu.target.path) },
            { key: "copy", label: "复制文件名", onSelect: () => copyName(menu.target.filename) },
            ...(onMoveImage
              ? [{ key: "move", label: "移动到…", onSelect: () => onMoveImage(menu.target) }]
              : []),
            ...(onDeleteImage
              ? [{ key: "delete", label: "删除到回收站", onSelect: () => onDeleteImage(menu.target) }]
              : []),
          ]}
        />
      )}
    </div>
  );
}
