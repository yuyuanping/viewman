import type { Image } from "../types";
import { ImageCard } from "./ImageCard";
import { useGridWindow } from "../hooks/useGridWindow";

interface ImageGridProps {
  images: Image[];
  duplicateIds?: Set<string>;
  /** 相似图检测命中的条目（pHash）：琥珀色高亮 */
  similarIds?: Set<string>;
  selectMode?: boolean;
  selectedIds?: Set<string>;
  onToggleSelect?: (image: Image) => void;
  onOpen: (image: Image) => void;
  onScanDirectory: () => void;
  onDeleted: () => void;
  onMoved: () => void;
}

export function ImageGrid({ images, duplicateIds, similarIds, selectMode, selectedIds, onToggleSelect, onOpen, onScanDirectory, onDeleted, onMoved }: ImageGridProps) {
  // 与 CSS .image-tiles 的 minmax/gap 对齐；900px 断点换窄屏值
  const isNarrow = typeof window !== "undefined" && window.innerWidth <= 900;
  const { onScroll, viewportRef, probeRef, slice, padTop, padBottom } =
    useGridWindow(images.length, isNarrow ? 120 : 150, isNarrow ? 10 : 14);

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

  const [start, end] = slice;

  return (
    <div className="flex-1 overflow-y-auto" ref={viewportRef} onScroll={onScroll}>
      {/* 探针卡：绝对定位到屏外，量真实高度；不在网格内占位 */}
      <div ref={probeRef} className="absolute overflow-hidden pointer-events-none" style={{ width: 150, left: -9999, top: 0 }} aria-hidden="true">
        <div className="image-tiles" style={{ display: "grid", gridTemplateColumns: "150px" }}>
          <ImageCard
            image={images[0]}
            onOpen={() => undefined}
            onDeleted={() => undefined}
            onMoved={() => undefined}
          />
        </div>
      </div>
      <div className="image-tiles">
        {padTop > 0 && <div style={{ height: padTop, gridColumn: "1 / -1" }} aria-hidden="true" />}
        {images.slice(start, end).map(image => (
          <ImageCard
            key={image.id}
            image={image}
            duplicate={duplicateIds?.has(image.id) ?? false}
            similar={similarIds?.has(image.id) ?? false}
            selectMode={selectMode}
            selected={selectedIds?.has(image.id) ?? false}
            onToggleSelect={onToggleSelect}
            onOpen={onOpen}
            onDeleted={onDeleted}
            onMoved={onMoved}
          />
        ))}
        {padBottom > 0 && <div style={{ height: padBottom, gridColumn: "1 / -1" }} aria-hidden="true" />}
      </div>
    </div>
  );
}
