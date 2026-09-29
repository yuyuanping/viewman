import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { justifiedRows, cardAspect } from "../justifiedLayout";

interface JustifiedGridProps<T> {
  items: T[];
  /** 每个条目的宽高比（宽/高；解不出尺寸时给 1）。传稳定引用，否则布局缓存每次渲染都失效 */
  aspectOf: (item: T) => number;
  keyOf: (item: T) => string;
  /** 条目内容：imageH 为该行图片区高度，条目内文字块高度须与 textH 一致 */
  renderItem: (item: T, imageH: number) => ReactNode;
  /** 条目下方文字块高度：行距 = imageH + textH + gap */
  textH: number;
  /** 目标行高：面板里卡片比主库小 */
  targetImageH?: number;
  gap?: number;
  className?: string;
  /**
   * 容器宽度：多个分组共享同一个滚动容器时由调用方量一次传进来，
   * 免得每个分组各挂一个 ResizeObserver（分组数上千时打开即卡死）。
   * 不传就自测（单列表面板用）。
   */
  width?: number;
}

/**
 * 谷歌相册式两端对齐布局的展示组件（非虚拟化）：
 * 行位置/高度由 justifiedLayout 算出，行内绝对定位、条目宽度按比例分配。
 * 图片库主网格（万级条目，需要按行虚拟滚动）走 ImageGrid 自己的实现；
 * 这里给分组审阅、模板匹配这类条目量可控的面板复用同一套视觉。
 */
export function JustifiedGrid<T>({ items, aspectOf, keyOf, renderItem, textH, targetImageH = 140, gap = 10, className, width: widthProp }: JustifiedGridProps<T>) {
  const [ownWidth, setOwnWidth] = useState(0);
  const elRef = useRef<HTMLDivElement | null>(null);

  const containerRef = useCallback((el: HTMLDivElement | null) => {
    elRef.current = el;
  }, []);

  useEffect(() => {
    if (widthProp !== undefined) return;
    const el = elRef.current;
    if (!el) return;
    const sync = () => setOwnWidth(el.clientWidth);
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(el);
    return () => ro.disconnect();
  }, [widthProp]);

  const width = widthProp ?? ownWidth;
  // aspects 并进同一个 memo：依赖里的 aspectOf/items 任一变化才重算
  const layout = useMemo(
    () => justifiedRows(items.map(aspectOf), width, targetImageH, gap, textH),
    [items, aspectOf, width, targetImageH, gap, textH],
  );

  return (
    <div ref={containerRef} className={`relative${className ? ` ${className}` : ""}`} style={{ height: layout.totalHeight }}>
      {layout.rows.map(row => (
        <div
          key={row.start}
          className="absolute left-0 right-0 flex overflow-hidden"
          style={{ top: row.top, height: row.imageH + textH, columnGap: gap }}
        >
          {items.slice(row.start, row.start + row.count).map(item => (
            <div key={keyOf(item)} style={{ width: row.imageH * cardAspect(aspectOf(item)) }}>
              {renderItem(item, row.imageH)}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
