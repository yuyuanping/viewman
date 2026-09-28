import { useCallback, useEffect, useRef, useState } from "react";

// 画面旋转：每点一次顺时针 90°；90/270 时按旋转后的外接框重新约束视频。
// resetKey 变化（换视频）时角度归位
export function useVideoRotation(resetKey?: string) {
  const fitBoxRef = useRef<HTMLDivElement>(null);
  const [rot, setRot] = useState(0);
  const rotateFrame = useCallback(() => setRot(r => (r + 90) % 360), []);

  useEffect(() => {
    setRot(0);
  }, [resetKey]);

  const [fitBox, setFitBox] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = fitBoxRef.current;
    if (!el) return;
    const measure = () => setFitBox({ w: el.clientWidth, h: el.clientHeight });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);

  const swapped = rot === 90 || rot === 270;
  const videoStyle = swapped && fitBox.w > 0
    ? { transform: `rotate(${rot}deg)`, maxWidth: `${fitBox.h}px`, maxHeight: `${fitBox.w}px` }
    : { transform: `rotate(${rot}deg)` };

  return { fitBoxRef, rot, rotateFrame, videoStyle };
}
