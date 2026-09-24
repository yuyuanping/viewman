import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { Image } from "../types";
import { formatFileSize, formatResolution } from "../utils";

interface ImageViewerProps {
  images: Image[];
  index: number;
  onNavigate: (index: number) => void;
  onClose: () => void;
  /** 删除由父层负责：删完后决定是前进到下一张还是关闭 */
  onDelete: (image: Image) => Promise<void>;
}

/** 滚轮每档缩放倍率；缩放范围 clamp 在 10%–800% */
const WHEEL_ZOOM = 1.15;
const MIN_SCALE = 0.1;
const MAX_SCALE = 8;
/** stage 左右内边距合计（styles/index.css: padding 0 28px） */
const STAGE_PAD_X = 56;
/** 幻灯片档位（秒/张） */
const SLIDE_DELAYS = [2, 5, 10] as const;

const clampScale = (v: number) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, v));

/** 全屏查看当前列表中的图片：← → 翻页，Esc 关闭，滚轮缩放，拖拽平移，R 旋转，0 适应窗口 */
export function ImageViewer({ images, index, onNavigate, onClose, onDelete }: ImageViewerProps) {
  // zoom=null 表示"适应窗口"；数字是相对原图的缩放倍率
  const [zoom, setZoom] = useState<number | null>(null);
  const [rotation, setRotation] = useState(0);
  const [broken, setBroken] = useState(false);
  const [stageSize, setStageSize] = useState({ w: 0, h: 0 });
  // 库里没有尺寸（ffprobe 未探测）时，用图片自身解码出的自然尺寸兜底
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  // 幻灯片：null=关，数字=每张停留秒数
  const [slideDelay, setSlideDelay] = useState<number | null>(null);
  // 拖拽平移：按下时记下指针与滚动位置，move 时按位移反推滚动
  const dragRef = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);
  const image = images[index];

  const go = useCallback((delta: number) => {
    const next = index + delta;
    if (next >= 0 && next < images.length) onNavigate(next);
  }, [index, images.length, onNavigate]);

  // 翻页时重置视图状态并回到滚动原点（适应窗口后 margin:auto 自动居中）
  useEffect(() => { setZoom(null); setRotation(0); }, [index]);
  useLayoutEffect(() => {
    const st = stageRef.current;
    if (st) { st.scrollLeft = 0; st.scrollTop = 0; }
  }, [index]);

  useEffect(() => setBroken(false), [image?.path]);
  useEffect(() => setNatural(null), [image?.path]);

  // stage 尺寸跟随窗口变化，供"适应窗口"计算
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const sync = () => setStageSize({ w: stage.clientWidth, h: stage.clientHeight });
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(stage);
    return () => ro.disconnect();
  }, []);

  // 旋转 90°/270° 时显示宽高互换；库里没尺寸时用解码出的自然尺寸
  const rawW = image?.width ?? natural?.w ?? 0;
  const rawH = image?.height ?? natural?.h ?? 0;
  const displayW = rotation % 180 === 90 ? rawH : rawW;
  const displayH = rotation % 180 === 90 ? rawW : rawH;

  // 适应窗口的倍率：只缩不放（≤1），宽高都塞进可视区
  const fitScale = useMemo(() => {
    if (!displayW || !displayH || !stageSize.w || !stageSize.h) return 1;
    const innerW = Math.max(1, stageSize.w - STAGE_PAD_X);
    return Math.min(innerW / displayW, stageSize.h / displayH, 1);
  }, [stageSize, displayW, displayH]);

  const scale = zoom ?? fitScale;

  // 滚轮缩放：围绕指针把图上同一点留在原地；旋转态下退化为居中缩放。
  // 原生非被动监听——React 合成 wheel 是被动的，preventDefault 会被忽略
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const base = zoom ?? fitScale;
      const factor = e.deltaY < 0 ? WHEEL_ZOOM : 1 / WHEEL_ZOOM;
      const next = clampScale(base * factor);
      if (next === base) return;
      const rect = stage.getBoundingClientRect();
      let relX = 0;
      let relY = 0;
      if (rotation % 180 === 0) {
        relX = (e.clientX - rect.left - stage.clientWidth / 2) / base;
        relY = (e.clientY - rect.top - stage.clientHeight / 2) / base;
      }
      setZoom(next);
      // 双 rAF 等新尺寸提交布局后再校正滚动，让缩放点仍在指针下
      requestAnimationFrame(() => requestAnimationFrame(() => {
        const st = stageRef.current;
        if (!st) return;
        st.scrollLeft = st.scrollWidth / 2 + relX * next - st.clientWidth / 2;
        st.scrollTop = st.scrollHeight / 2 + relY * next - st.clientHeight / 2;
      }));
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, [zoom, fitScale, rotation]);

  const toggleActualSize = useCallback(() => {
    setZoom(prev => (prev === 1 ? null : 1));
  }, []);

  const handleDragStart = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const stage = stageRef.current;
    if (!stage) return;
    // 画布不超出可视区时拖不动（也无需拖），直接不进入拖拽
    if (stage.scrollWidth <= stage.clientWidth && stage.scrollHeight <= stage.clientHeight) return;
    dragRef.current = { x: e.clientX, y: e.clientY, left: stage.scrollLeft, top: stage.scrollTop };
    stage.setPointerCapture?.(e.pointerId);
  }, []);

  const handleDragMove = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    const stage = stageRef.current;
    if (!d || !stage) return;
    stage.scrollLeft = d.left - (e.clientX - d.x);
    stage.scrollTop = d.top - (e.clientY - d.y);
  }, []);

  const handleDragEnd = useCallback(() => { dragRef.current = null; }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { onClose(); }
      else if (e.key === "ArrowLeft") { setSlideDelay(null); go(-1); }
      else if (e.key === "ArrowRight") { setSlideDelay(null); go(1); }
      else if (e.key === "1" || e.key === "f") toggleActualSize();
      else if (e.key === "r" || e.key === "R") setRotation(r => (r + 90) % 360);
      else if (e.key === "0") setZoom(null);
      else if (e.key === " ") {
        e.preventDefault();
        setSlideDelay(d => (d === null ? SLIDE_DELAYS[0] : null));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [go, onClose, toggleActualSize]);

  // 幻灯片：定时翻页；到末尾一张即停（不循环）
  useEffect(() => {
    if (slideDelay === null) return;
    const t = setTimeout(() => {
      if (index < images.length - 1) onNavigate(index + 1);
      else setSlideDelay(null);
    }, slideDelay * 1000);
    return () => clearTimeout(t);
  }, [slideDelay, index, images.length, onNavigate]);

  if (!image) return null;

  const handleDelete = async () => {
    if (!confirm(`确定要删除 "${image.filename}" 到回收站？`)) return;
    try {
      await onDelete(image);
    } catch (err) {
      alert("删除失败: " + err);
    }
  };

  const zoomLabel = zoom === null ? "适应窗口" : zoom === 1 ? "1:1 原始尺寸" : `${Math.round(zoom * 100)}%`;

  return (
    <div className="image-viewer fixed inset-0 z-40 flex flex-col" role="dialog" aria-modal="true" aria-label={`查看图片 ${image.filename}`}>
      <div className="flex items-center gap-3 px-4 py-3 shrink-0">
        <span className="text-sm text-gray-200 truncate" title={image.path}>{image.filename}</span>
        <span className="text-xs text-gray-400 shrink-0 tabular-nums">
          {formatResolution(image.width, image.height)} · {formatFileSize(image.file_size)} · {index + 1}/{images.length}
        </span>
        <span className="toolbar-spacer" />
        <button type="button" className="toolbar-chip" onClick={() => setRotation(r => (r + 90) % 360)} title="旋转 90°（R）">
          旋转
        </button>
        {slideDelay === null ? (
          <button
            type="button"
            className="toolbar-chip"
            onClick={() => setSlideDelay(SLIDE_DELAYS[0])}
            disabled={images.length < 2}
            title="幻灯片播放（空格开始/暂停）"
          >
            幻灯片
          </button>
        ) : (
          <button
            type="button"
            className="toolbar-chip"
            onClick={() => {
              const i = SLIDE_DELAYS.indexOf(slideDelay as (typeof SLIDE_DELAYS)[number]);
              setSlideDelay(SLIDE_DELAYS[(i + 1) % SLIDE_DELAYS.length]);
            }}
            title="切换幻灯片速度，空格暂停"
          >
            幻灯片 {slideDelay}s ▸
          </button>
        )}
        <button type="button" className="toolbar-chip" onClick={toggleActualSize} title="1:1 原始尺寸（1 / f），0 或再次点击回到适应窗口">
          {zoom === 1 ? "适应窗口" : zoomLabel}
        </button>
        <button
          type="button"
          className="toolbar-chip"
          onClick={() => revealItemInDir(image.path).catch(err => alert("打开文件所在位置失败: " + err))}
          title="在资源管理器中定位该文件"
        >
          打开位置
        </button>
        <button type="button" className="toolbar-chip" onClick={handleDelete} title="移入回收站">
          删除
        </button>
        <button type="button" className="toolbar-chip" onClick={onClose} title="关闭（Esc）" aria-label="关闭查看器">
          ✕
        </button>
      </div>

      <div
        className="image-viewer-stage flex-1"
        ref={stageRef}
        onPointerDown={handleDragStart}
        onPointerMove={handleDragMove}
        onPointerUp={handleDragEnd}
        onPointerCancel={handleDragEnd}
      >
        {broken ? (
          <p className="text-gray-400 text-sm p-6">该图片当前无法显示（文件可能已被移动、删除或格式不受支持）。</p>
        ) : (
          <div
            className="image-viewer-canvas"
            style={{ width: displayW * scale, height: displayH * scale }}
          >
            <img
              src={convertFileSrc(image.path)}
              alt={image.filename}
              draggable={false}
              style={{ transform: `translate(-50%, -50%) rotate(${rotation}deg) scale(${scale})` }}
              onDoubleClick={toggleActualSize}
              onLoad={(e) => {
                const el = e.currentTarget;
                if (!image.width || !image.height) setNatural({ w: el.naturalWidth, h: el.naturalHeight });
              }}
              onError={() => setBroken(true)}
            />
          </div>
        )}
      </div>

      <div className="flex items-center justify-center gap-4 py-3 shrink-0">
        <button type="button" className="toolbar-chip" disabled={index === 0} onClick={() => go(-1)} aria-label="上一张（←）">
          ← 上一张
        </button>
        <button type="button" className="toolbar-chip" disabled={index >= images.length - 1} onClick={() => go(1)} aria-label="下一张（→）">
          下一张 →
        </button>
      </div>
    </div>
  );
}
