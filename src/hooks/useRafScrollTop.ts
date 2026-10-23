import { useCallback, useEffect, useRef } from "react";

/**
 * 滚动位置的 rAF 节流：高频 scroll 事件只记最新位置，下一帧统一交给 setState。
 *
 * 图片网格（按行虚拟化）与视频网格（按行窗口化）都要每帧知道 scrollTop；
 * 不节流的话一次滑动能触发几十次重渲染，窗口内的卡片全部跟着 diff。
 * 卸载时取消挂起的那一帧，避免对已卸载组件 setState。
 */
export function useRafScrollTop(setScrollTop: (top: number) => void) {
  const rafId = useRef(0);
  const pendingTop = useRef<number | null>(null);
  useEffect(() => () => {
    if (rafId.current) cancelAnimationFrame(rafId.current);
  }, []);
  return useCallback((e: React.UIEvent<HTMLDivElement>) => {
    pendingTop.current = e.currentTarget.scrollTop;
    if (rafId.current) return;
    rafId.current = requestAnimationFrame(() => {
      rafId.current = 0;
      if (pendingTop.current !== null) setScrollTop(pendingTop.current);
    });
  }, [setScrollTop]);
}
