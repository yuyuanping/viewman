import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Image } from "../types";
import { ImageCard } from "./ImageCard";
import { EmptyState } from "./EmptyState";
import { justifiedRows, justifiedWindow, cardAspect, CARD_TEXT_H } from "../justifiedLayout";
import { useRafScrollTop } from "../hooks/useRafScrollTop";

interface ImageGridProps {
  images: Image[];
  duplicateIds?: Set<string>;
  /** 动图检测命中的条目（多帧）：紫色高亮 */
  animatedIds?: Set<string>;
  /** 以某张图为模板找库内相似图（右键菜单入口）；不给就不显示菜单项。
   *  调用方多是内联箭头、每次渲染换新引用，网格按 id 缓存稳定包装再交给卡片 */
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
export function ImageGrid({ images, duplicateIds, animatedIds, onFindSimilar, selectMode, selectedIds, onToggleSelect, onOpen, onScanDirectory, onDeleted, onMoved, resetKey }: ImageGridProps) {
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [viewportW, setViewportW] = useState(0);
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null);
  const viewportRefObj = useRef<HTMLDivElement | null>(null);
  const findSimilarCache = useRef(new Map<string, (image: Image) => void>());

  const viewportRef = useCallback((el: HTMLDivElement | null) => {
    viewportRefObj.current = el;
    setViewportEl(el);
  }, []);
  // 滚动 rAF 节流见 useRafScrollTop（视频网格共用同一个）
  const onScroll = useRafScrollTop(setScrollTop);

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

  // 宽高比增量缓存：视图每次 setView 换整数组，以前全量 map 重算 O(N)；
  // 按 id 缓存后只有新图/尺寸变化的才重算，切目录回顶部不再掉帧
  const aspectCache = useRef(new Map<string, number>());
  const aspects = useMemo(() => {
    const cache = aspectCache.current;
    // 视图缩小时顺手清掉已不在视图的 key，Map 不无限胀
    if (cache.size > images.length * 2) {
      const alive = new Set(images.map((img) => img.id));
      for (const key of cache.keys()) {
        if (!alive.has(key)) cache.delete(key);
      }
    }
    return images.map((img) => {
      const hit = cache.get(img.id);
      const next = img.width && img.height ? img.width / img.height : 1;
      // 尺寸不变沿用缓存（NaN 保护：宽高缺失恒为 1）
      if (hit !== undefined && img.width && img.height) {
        // 粗略校验：缓存值与当前值差异极小则沿用，避免浮点抖动
        if (Math.abs(hit - next) < 1e-9) return hit;
      }
      cache.set(img.id, next);
      return next;
    });
  }, [images]);
  const layout = useMemo(
    () => justifiedRows(aspects, viewportW, TARGET_IMAGE_H, GAP, CARD_TEXT_H),
    [aspects, viewportW],
  );

  // 卡片勾选回调：网格里每张卡都绑一个「第几张 + shift」的闭包。
  // 闭包一旦生成就稳定（ref 复用），卡片 memo 才生效；视图重排/增减后
  // 闭包仍指向生成时的下标，所以统一经 ref 实时查当前下标
  const indexOf = useMemo(() => {
    const m = new Map<string, number>();
    images.forEach((img, i) => m.set(img.id, i));
    return m;
  }, [images]);
  const indexRef = useRef(indexOf);
  indexRef.current = indexOf;
  // 上层传入的回调本身每次渲染都会换新（多是内联箭头），闭包里别直接捕获它
  const onToggleSelectRef = useRef(onToggleSelect);
  onToggleSelectRef.current = onToggleSelect;
  const onFindSimilarRef = useRef(onFindSimilar);
  onFindSimilarRef.current = onFindSimilar;
  // 缓存只增不减会随翻遍全库无限长：超过视图两倍时只保留视图里还在的 id。
  // 判据必须是「这个 id 还在视图里吗」，不能按当前下标判——清一次之后
  // 命中缓存的路径不再进来，把判断写错成 indexOf.get(id) === undefined
  // 也会让缓存彻底失效、每帧重建闭包
  const findSimilarFor = useCallback((id: string) => {
    const cache = findSimilarCache.current;
    let fn = cache.get(id);
    if (!fn) {
      fn = (img: Image) => onFindSimilarRef.current?.(img);
      cache.set(id, fn);
      if (cache.size > images.length * 2) {
        for (const key of cache.keys()) if (!indexOf.has(key)) cache.delete(key);
      }
    }
    return fn;
  }, [images.length, indexOf]);
  const toggleCache = useRef(new Map<string, (shiftKey: boolean) => void>());
  const toggleFor = useCallback((id: string) => {
    const cache = toggleCache.current;
    let fn = cache.get(id);
    if (!fn) {
      fn = (shiftKey: boolean) => onToggleSelectRef.current?.(indexRef.current.get(id) ?? -1, shiftKey);
      cache.set(id, fn);
      if (cache.size > images.length * 2) {
        for (const key of cache.keys()) if (!indexOf.has(key)) cache.delete(key);
      }
    }
    return fn;
  }, [images.length, indexOf]);

  if (images.length === 0) {
    return (
      <EmptyState
        icon={
          <svg width="30" height="30" viewBox="0 0 24 24" stroke="currentColor" fill="none" strokeWidth="1.5" aria-hidden="true">
            <rect x="3" y="4" width="18" height="16" rx="3" />
            <circle cx="8.5" cy="9.5" r="1.5" />
            <path d="m4 17 5-5 4 4 3-2 4 4" />
          </svg>
        }
        title="这里还没有图片"
        hint="扫描图片目录加入图片库，或调整搜索条件。"
        action={
          <button onClick={onScanDirectory} className="btn-primary px-4 py-2 text-sm">
            扫描图片目录
          </button>
        }
      />
    );
  }

  // 首帧视口还没量到宽度：渲染前 60 张方格探针（与旧网格同款），
  // ResizeObserver 量到后下一帧自动切成对齐布局
  if (viewportW <= 0) {
    return (
      <div className="flex-1 overflow-y-auto" ref={viewportRef} onScroll={onScroll}>
        <div className="image-tiles">
          {images.slice(0, 60).map((image) => (
            <ImageCard
              key={image.id}
              image={image}
              duplicate={duplicateIds?.has(image.id) ?? false}
              animated={animatedIds?.has(image.id) ?? false}
              onFindSimilar={onFindSimilar ? findSimilarFor(image.id) : undefined}
              selectMode={selectMode}
              selected={selectedIds?.has(image.id) ?? false}
              onToggleSelect={toggleFor(image.id)}
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
            {images.slice(row.start, row.start + row.count).map((image) => {
              const aspect = aspects[indexOf.get(image.id) ?? row.start];
              return (
                <div key={image.id} style={{ width: row.imageH * cardAspect(aspect) }}>
                  <ImageCard
                    image={image}
                    imageHeight={row.imageH}
                    duplicate={duplicateIds?.has(image.id) ?? false}
                    animated={animatedIds?.has(image.id) ?? false}
                    onFindSimilar={onFindSimilar ? findSimilarFor(image.id) : undefined}
                    selectMode={selectMode}
                    selected={selectedIds?.has(image.id) ?? false}
                    onToggleSelect={toggleFor(image.id)}
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
