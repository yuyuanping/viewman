import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 卡片网格虚拟化：只渲染可视区 ±N 行的卡片，其余用空白撑高保持滚动条真实。
 *
 * 为什么手写：项目零虚拟化依赖，网格是 CSS `repeat(auto-fill, minmax(minCol, 1fr))`，
 * 列数随窗口宽变、行高固定（卡片 aspect-ratio + 恒定文案行），这两个量都可推导：
 * - 列数 = floor((容器宽 + gap) / (minCol + gap))，与 CSS auto-fill 公式一致
 * - 行高 = 探针卡实测高度（ResizeObserver 跟踪响应式/字体变化）
 *
 * 探针未就绪的首帧退回"渲染前 60 张"，量到行高后自动切窗口模式，
 * 用户在首屏看不出差异。
 */
export interface GridWindowState {
  /** 挂到滚动容器 */
  onScroll: (e: React.UIEvent<HTMLDivElement>) => void;
  viewportRef: React.RefObject<HTMLDivElement | null>;
  /** 挂到一个只含一张卡的探针行（绝对定位、零占位，不影响布局） */
  probeRef: React.RefObject<HTMLDivElement | null>;
  /** 当前应渲染的条目下标区间 [start, end) */
  slice: [number, number];
  /** 窗口前的空白高度（px），撑住未渲染部分 */
  padTop: number;
  /** 窗口后的空白高度（px） */
  padBottom: number;
  /** 探针是否已量到行高（未就绪时 slice 固定为前 60 条） */
  ready: boolean;
}

export function useGridWindow(
  total: number,
  minColPx: number,
  gapPx: number,
  overscanRows = 4,
): GridWindowState {
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [viewportW, setViewportW] = useState(0);
  const [cardH, setCardH] = useState(0);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const probeRef = useRef<HTMLDivElement | null>(null);

  const onScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    setScrollTop(e.currentTarget.scrollTop);
  }, []);

  // 视口尺寸跟踪
  useEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const sync = () => {
      setViewportH(el.clientHeight);
      setViewportW(el.clientWidth);
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 单卡高度探针
  useEffect(() => {
    const probe = probeRef.current;
    if (!probe) return;
    const measure = () => {
      const h = probe.offsetHeight;
      if (h > 0) setCardH(prev => (Math.abs(prev - h) > 0.5 ? h : prev));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(probe);
    return () => ro.disconnect();
  }, []);

  const columns = columnsOf(viewportW, minColPx, gapPx);
  const ready = cardH > 0 && viewportH > 0;

  if (!ready) {
    return {
      onScroll, viewportRef, probeRef,
      slice: [0, Math.min(total, 60)],
      padTop: 0, padBottom: 0, ready: false,
    };
  }

  const { start, end, padTop, padBottom } = gridWindowOf(
    total, columns, cardH, gapPx, scrollTop, viewportH, overscanRows,
  );

  return {
    onScroll, viewportRef, probeRef,
    slice: [start, end],
    padTop, padBottom, ready: true,
  };
}

/** 与 CSS repeat(auto-fill, minmax(minCol, 1fr)) 一致的列数推导（独立纯函数，可测） */
export function columnsOf(containerW: number, minCol: number, gap: number): number {
  if (containerW <= 0) return 1;
  return Math.max(1, Math.floor((containerW + gap) / (minCol + gap)));
}

/** 行窗口裁剪（纯函数）：给定滚动位置返回 [start, end) 下标与上下空白高度 */
export function gridWindowOf(
  total: number,
  columns: number,
  cardH: number,
  gap: number,
  scrollTop: number,
  viewportH: number,
  overscanRows: number,
): { start: number; end: number; padTop: number; padBottom: number } {
  if (columns < 1 || cardH <= 0) return { start: 0, end: Math.min(total, 60), padTop: 0, padBottom: 0 };
  const rowH = cardH + gap;
  const totalRows = Math.ceil(total / columns);
  const firstRow = Math.max(0, Math.floor(scrollTop / rowH));
  const lastRow = Math.min(totalRows - 1, Math.ceil((scrollTop + viewportH) / rowH));
  const startRow = Math.max(0, firstRow - overscanRows);
  const endRow = Math.min(totalRows, lastRow + 1 + overscanRows);
  return {
    start: startRow * columns,
    end: Math.min(total, endRow * columns),
    padTop: startRow * rowH,
    padBottom: Math.max(0, (totalRows - endRow) * rowH),
  };
}
