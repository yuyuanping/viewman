import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Image } from "../types";
import { ImageCard } from "./ImageCard";
import { justifiedRows, justifiedWindow, cardAspect, CARD_TEXT_H } from "../justifiedLayout";

interface ImageGridProps {
  images: Image[];
  duplicateIds?: Set<string>;
  /** 相似图检测命中的条目（pHash）：琥珀色高亮 */
  similarIds?: Set<string>;
  /** 动图检测命中的条目（多帧）：紫色高亮 */
  animatedIds?: Set<string>;
  /** 以某张图为模板找库内相似图（右键菜单入口）；不给就不显示菜单项 */
  onFindSimilar?: (image: Image) => void;
  selectMode?: boolean;
  selectedIds?: Set<string>;
  onToggleSelect?: (index: number, shiftKey: boolean) => void;
  onOpen: (image: Image) => void;
  onScanDirectory: () => void;
  onDeleted: (imageId: string) => void;
  onMoved: (imageId: string, newPath: string) => void;
  /** 目录/过滤切换时滚动归零 */
  resetKey: unknown;
}

/** 谷歌相册式布局参数：目标行高、行距/图距、可视区上下多渲染的距离 */
const TARGET_IMAGE_H = 200;
const GAP = 12;
const OVERSCAN_PX = 800;

/**
 * 图片网格：谷歌相册式两端对齐布局（每行按原始宽高比铺满容器宽），
 * 按行虚拟化——只渲染可视区 ±OVERSCAN_PX 内的行，行的位置与高度
 * 由 justifiedLayout 纯函数算出，行内绝对定位、行内 flex 排卡片。
 */
export function ImageGrid({ images, duplicateIds, similarIds, animatedIds, onFindSimilar, selectMode, selectedIds, onToggleSelect, onOpen, onScanDirectory, onDeleted, onMoved, resetKey }: ImageGridProps) {
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [viewportW, setViewportW] = useState(0);
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null);
  const viewportRefObj = useRef<HTMLDivElement | null>(null);

  const viewportRef = useCallback((el: HTMLDivElement | null) => {
    viewportRefObj.current = el;
    setViewportEl(el);
  }, []);
  const onScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    setScrollTop(e.currentTarget.scrollTop);
  }, []);

  // 视口尺寸跟踪；元素刚挂上时以它的真实滚动位置为准（重挂载后 DOM 归零，
  // 而 scrollTop state 还留着旧值，会让窗口算到列表中段去）
  useEffect(() => {
    if (!viewportEl) return;
    const sync = () => {
      setViewportH(viewportEl.clientHeight);
      setViewportW(viewportEl.clientWidth);
      setScrollTop(viewportEl.scrollTop);
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(viewportEl);
    return () => ro.disconnect();
  }, [viewportEl]);

  // resetKey 变化（切换目录/过滤）：滚动归零，符合"切目录回顶部"的直觉
  useEffect(() => {
    const el = viewportRefObj.current;
    if (el && el.scrollTop > 0) el.scrollTop = 0;
    setScrollTop(0);
  }, [resetKey]);

  const aspects = useMemo(
    () => images.map(img => (img.width && img.height ? img.width / img.height : 1)),
    [images],
  );
  const layout = useMemo(
    () => justifiedRows(aspects, viewportW, TARGET_IMAGE_H, GAP, CARD_TEXT_H),
    [aspects, viewportW],
  );

  if (images.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-500">
        <div className="text-center">
          <div className="mx-auto mb-5 w-16 h-16 rounded-2xl border border-white/10 bg-gray-800 grid place-items-center text-blue-500">
            <svg width="28" height="28" viewBox="0 0 24 24" stroke="currentColor" fill="none" strokeWidth="1.5" aria-hidden="true">
              <rect x="3" y="4" width="18" height="16" rx="3" />
              <circle cx="8.5" cy="9.5" r="1.5" />
              <path d="m4 17 5-5 4 4 3-2 4 4" />
            </svg>
          </div>
          <p className="text-lg text-gray-200">这里还没有图片</p>
          <p className="text-sm mt-2 mb-4">扫描图片目录加入图片库，或调整搜索条件。</p>
          <button onClick={onScanDirectory} className="bg-blue-600 hover:bg-blue-500 py-2 px-4 rounded-xl text-sm font-medium">
            扫描图片目录
          </button>
        </div>
      </div>
    );
  }

  // 首帧视口还没量到宽度：渲染前 60 张方格探针（与旧网格同款），
  // ResizeObserver 量到后下一帧自动切成对齐布局
  if (viewportW <= 0) {
    return (
      <div className="flex-1 overflow-y-auto" ref={viewportRef} onScroll={onScroll}>
        <div className="image-tiles">
          {images.slice(0, 60).map((image, at) => (
            <ImageCard
              key={image.id}
              image={image}
              duplicate={duplicateIds?.has(image.id) ?? false}
              similar={similarIds?.has(image.id) ?? false}
              animated={animatedIds?.has(image.id) ?? false}
              onFindSimilar={onFindSimilar}
              selectMode={selectMode}
              selected={selectedIds?.has(image.id) ?? false}
              onToggleSelect={shiftKey => onToggleSelect?.(at, shiftKey)}
              onOpen={onOpen}
              onDeleted={onDeleted}
              onMoved={onMoved}
            />
          ))}
        </div>
      </div>
    );
  }

  const [startRow, endRow] = justifiedWindow(layout.rows, scrollTop, viewportH, OVERSCAN_PX);

  return (
    <div className="flex-1 overflow-y-auto" ref={viewportRef} onScroll={onScroll}>
      <div className="relative" style={{ height: layout.totalHeight }}>
        {layout.rows.slice(startRow, endRow).map(row => (
          <div
            key={row.start}
            className="absolute left-0 right-0 flex overflow-hidden"
            style={{ top: row.top, height: row.imageH + CARD_TEXT_H, columnGap: GAP }}
          >
            {images.slice(row.start, row.start + row.count).map((image, at) => {
              const index = row.start + at;
              return (
                <div key={image.id} style={{ width: row.imageH * cardAspect(aspects[index]) }}>
                  <ImageCard
                    image={image}
                    imageHeight={row.imageH}
                    duplicate={duplicateIds?.has(image.id) ?? false}
                    similar={similarIds?.has(image.id) ?? false}
                    animated={animatedIds?.has(image.id) ?? false}
                    onFindSimilar={onFindSimilar}
                    selectMode={selectMode}
                    selected={selectedIds?.has(image.id) ?? false}
                    onToggleSelect={shiftKey => onToggleSelect?.(index, shiftKey)}
                    onOpen={onOpen}
                    onDeleted={onDeleted}
                    onMoved={onMoved}
                  />
                </div>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}
