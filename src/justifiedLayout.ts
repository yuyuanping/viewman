/**
 * 谷歌相册式「两端对齐」布局（纯函数）：
 * 每行图片保留原始宽高比，整行缩放到恰好铺满容器宽度，行高随内容变化；
 * 最后一行不强行拉伸。卡片下方有恒定高度的文字区，计入行距。
 */

/** 卡片图片区下方的文字块高度：布局行距与 ImageCard 的文字块共用这一个常量 */
export const CARD_TEXT_H = 52;

/** 宽高比夹取范围：全景图不至于独占一整屏高的行，竖图不至于占满整屏宽 */
const MIN_ASPECT = 0.5;
const MAX_ASPECT = 4;

/** 单行布局：top 为行顶偏移，imageH 为该行图片区高度，bottom 含文字区 */
export interface JustifiedRow {
  top: number;
  imageH: number;
  start: number;
  count: number;
  bottom: number;
}

export interface JustifiedLayoutResult {
  rows: JustifiedRow[];
  /** 列表总高度（最后一行图片区 + 文字区，不含行距尾巴） */
  totalHeight: number;
}

/** 卡片实际使用的宽高比：行高预算与卡片宽度必须用同一个夹取，否则全景图会溢出容器 */
export function cardAspect(a: number): number {
  return Math.min(MAX_ASPECT, Math.max(MIN_ASPECT, a));
}

/**
 * 贪心成行：往当前行加图片，加到"再加一张行高压到目标以下"为止——
 * 此时行高恰好 ≥ 目标，行宽恰好等于容器宽。末行不拉伸，行高封顶目标值；
 * 单张特宽/特长的图行高再封顶 2 倍目标，宁可不满宽也不撑爆一屏。
 */
export function justifiedRows(
  aspects: number[],
  containerWidth: number,
  targetImageH: number,
  gap: number,
  textH: number,
): JustifiedLayoutResult {
  const rows: JustifiedRow[] = [];
  if (containerWidth <= 0 || aspects.length === 0) {
    return { rows, totalHeight: 0 };
  }
  let top = 0;
  let i = 0;
  while (i < aspects.length) {
    const start = i;
    let sumAspect = 0;
    let imageH = targetImageH;
    while (i < aspects.length) {
      const inRow = i - start;
      const tryH = (containerWidth - gap * inRow) / (sumAspect + cardAspect(aspects[i]));
      if (inRow > 0 && tryH < targetImageH) break;
      sumAspect += cardAspect(aspects[i]);
      imageH = tryH;
      i++;
    }
    const isLast = i >= aspects.length;
    if (isLast) imageH = Math.min(imageH, targetImageH);
    imageH = Math.min(imageH, targetImageH * 2);
    rows.push({ top, imageH, start, count: i - start, bottom: top + imageH + textH });
    top += imageH + textH + gap;
  }
  return { rows, totalHeight: Math.max(0, top - gap) };
}

/**
 * 行窗口裁剪（纯函数）：返回与可视区 ±overscan 相交的行下标区间 [start, end)。
 * 行数在几万级，线性扫足够快，不需要二分。
 */
export function justifiedWindow(
  rows: JustifiedRow[],
  scrollTop: number,
  viewportH: number,
  overscan: number,
): [number, number] {
  let start = 0;
  while (start < rows.length && rows[start].bottom <= scrollTop - overscan) start++;
  let end = start;
  while (end < rows.length && rows[end].top < scrollTop + viewportH + overscan) end++;
  return [start, end];
}
