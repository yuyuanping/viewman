import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 卡片网格虚拟化：只渲染可视区 ±N 行的卡片，其余用空白撑高保持滚动条真实。
 *
 * 设计要点（都有历史教训）：
 * 1. 列数/列宽/行距不自己推导——直接读真实网格元素的 computed style
 *    （grid-template-columns / row-gap）。CSS 是唯一真相：媒体查询断点、
 *    auto-fill 变化、未来改样式都不需要同步两处公式。
 * 2. 行高 = 真实网格里第一张真实卡的实测锚高（卡片 aspect-ratio + 文案行恒定，
 *    同行所有卡等高），撑高 pad 块用 data-pad 标记跳过，ResizeObserver
 *    跟踪窗口宽/字体变化自动重测。
 * 3. ref 用 callback ref + state：网格元素可能比 hook 晚出现（列表先空后有），
 *    useRef + 空依赖 effect 会永远错过它，observer 一次都挂不上 → 行重量不到
 *    → 停在"只渲染前 60 条"且滚不动（历史 bug：启动后首屏看不全库）。
 * 4. scrollTop 防残留：切换目录/过滤导致列表变短时，浏览器把 scrollTop
 *    夹回来需要一帧，窗口计算先按当前滚动位置夹到有效行区间，
 *    再叠 overscan——顺序错误会产生 start > end 的空窗口（历史 bug：
 *    网格永久空白，看起来像"切目录卡死"）。
 * 5. 探针未就绪（首帧）渲染前 60 条，量到行高后自动切窗口模式。
 * 6. resetKey 变化（切换目录/过滤）时滚动归零：既防残留 scrollTop 越界，
 *    也符合"切目录回顶部"的直觉。
 */

/** rowHeightOf 只依赖这几个量，用结构类型即可（真实 HTMLElement 天然满足，测试可传假对象） */
export interface RowAnchor {
  offsetTop: number;
  offsetHeight: number;
  dataset?: { pad?: string };
}

/**
 * 行高锚（纯函数）：跳过撑高占位（data-pad），取第一张真实卡为锚，
 * 往后第一张 offsetTop 更大的卡就在下一行，差值即行高；
 * 同排卡 offsetTop 相等（差 0 不是行高），只有一行时退回卡高 + row-gap。
 */
export function rowHeightOf(children: ArrayLike<RowAnchor>, gap: number): number {
  let first: RowAnchor | null = null;
  for (let i = 0; i < children.length; i++) {
    const c = children[i];
    if (c.dataset?.pad) continue;
    if (first === null) {
      first = c;
      continue;
    }
    if (c.offsetTop > first.offsetTop) return c.offsetTop - first.offsetTop;
  }
  return first ? first.offsetHeight + gap : 0;
}

export interface GridWindowState {
  /** 挂到滚动容器 */
  onScroll: (e: React.UIEvent<HTMLDivElement>) => void;
  /** callback ref：挂到滚动容器 */
  viewportRef: (el: HTMLDivElement | null) => void;
  /** callback ref：挂到真实网格元素（读 computed style 量列宽行距、行高） */
  gridRef: (el: HTMLDivElement | null) => void;
  /** 变化时滚动归零并重测（传"当前目录/过滤键"） */
  resetKey: unknown;
  /** 当前应渲染的条目下标区间 [start, end) */
  slice: [number, number];
  /** 窗口前的空白高度（px），撑住未渲染部分 */
  padTop: number;
  /** 窗口后的空白高度（px） */
  padBottom: number;
  /** 是否已在窗口模式（false = 首帧探针，渲染前 60 条） */
  ready: boolean;
}

export function useGridWindow(
  total: number,
  resetKey: unknown,
  overscanRows = 4,
): GridWindowState {
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const [viewportEl, setViewportEl] = useState<HTMLDivElement | null>(null);
  const [gridEl, setGridEl] = useState<HTMLDivElement | null>(null);
  const [gridGeom, setGridGeom] = useState({ columns: 1, rowH: 0 });
  const viewportRefObj = useRef<HTMLDivElement | null>(null);

  const viewportRef = useCallback((el: HTMLDivElement | null) => {
    viewportRefObj.current = el;
    setViewportEl(el);
  }, []);
  const gridRef = useCallback((el: HTMLDivElement | null) => {
    setGridEl(el);
  }, []);

  const onScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    setScrollTop(e.currentTarget.scrollTop);
  }, []);

  // 视口高度跟踪；元素刚挂上时以它的真实滚动位置为准（重挂载后 DOM 归零，
  // 而 scrollTop state 还留着旧值，会让窗口算到列表中段去）
  useEffect(() => {
    if (!viewportEl) return;
    const sync = () => {
      setViewportH(viewportEl.clientHeight);
      setScrollTop(viewportEl.scrollTop);
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(viewportEl);
    return () => ro.disconnect();
  }, [viewportEl]);

  // 网格几何测量：列数（从 grid-template-columns 数）+ 第一行实测高度
  // 浏览器把 grid-template-columns 解析成 "220px 220px ..."，直接用。
  const measureGrid = useCallback(() => {
    if (!gridEl || gridEl.children.length === 0) return;
    const cs = getComputedStyle(gridEl);
    const cols = cs.gridTemplateColumns
      ? cs.gridTemplateColumns.split(/\s+/).filter(Boolean).length
      : 1;
    if (cols < 1) return;
    const gap = parseFloat(cs.rowGap) || 0;
    const rowH = rowHeightOf(gridEl.children as unknown as ArrayLike<RowAnchor>, gap);
    if (rowH > 0) {
      setGridGeom(prev =>
        prev.columns === cols && Math.abs(prev.rowH - rowH) < 0.5 ? prev : { columns: cols, rowH },
      );
    }
  }, [gridEl]);

  useEffect(() => {
    if (!gridEl) return;
    measureGrid();
    const ro = new ResizeObserver(() => measureGrid());
    ro.observe(gridEl);
    return () => ro.disconnect();
  }, [measureGrid, gridEl]);

  // resetKey 变化：滚动归零并重测（防旧 scrollTop 越界 + 符合切目录回顶直觉）
  useEffect(() => {
    const el = viewportRefObj.current;
    if (el && el.scrollTop > 0) el.scrollTop = 0;
    setScrollTop(0);
    measureGrid();
  }, [resetKey, measureGrid]);

  const { columns, rowH } = gridGeom;
  const ready = rowH > 0 && viewportH > 0;

  if (!ready) {
    return {
      onScroll, viewportRef, gridRef, resetKey,
      slice: [0, Math.min(total, 60)],
      padTop: 0, padBottom: 0, ready: false,
    };
  }

  const { start, end, padTop, padBottom } = gridWindowOf(
    total, columns, rowH, scrollTop, viewportH, overscanRows,
  );

  return {
    onScroll, viewportRef, gridRef, resetKey,
    slice: [start, end],
    padTop, padBottom, ready: true,
  };
}

/**
 * 行窗口裁剪（纯函数）：给定滚动位置返回 [start, end) 下标与上下空白高度。
 *
 * 夹取顺序是正确性核心：先把可见行夹到 [0, totalRows-1]，再叠 overscan
 * 后重夹 endRow——保证任何 scrollTop（包括列表刚变短时的残留值）都得到
 * start <= end 的有效窗口。
 */
export function gridWindowOf(
  total: number,
  columns: number,
  rowH: number,
  scrollTop: number,
  viewportH: number,
  overscanRows: number,
): { start: number; end: number; padTop: number; padBottom: number } {
  if (columns < 1 || rowH <= 0) return { start: 0, end: Math.min(total, 60), padTop: 0, padBottom: 0 };
  const totalRows = Math.ceil(total / columns);
  const firstRow = Math.min(Math.max(0, Math.floor(scrollTop / rowH)), Math.max(0, totalRows - 1));
  const lastRow = Math.min(Math.max(0, Math.ceil((scrollTop + viewportH) / rowH)), Math.max(0, totalRows - 1));
  const startRow = Math.max(0, firstRow - overscanRows);
  const endRow = Math.min(totalRows, lastRow + 1 + overscanRows);
  return {
    start: startRow * columns,
    end: Math.min(total, endRow * columns),
    padTop: startRow * rowH,
    padBottom: Math.max(0, (totalRows - endRow) * rowH),
  };
}
