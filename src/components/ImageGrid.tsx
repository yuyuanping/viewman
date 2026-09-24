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
  onDeleted: (imageId: string) => void;
  onMoved: (imageId: string, newPath: string) => void;
  /** 目录/过滤切换时滚动归零并重测网格几何 */
  resetKey: unknown;
}

export function ImageGrid({ images, duplicateIds, similarIds, selectMode, selectedIds, onToggleSelect, onOpen, onScanDirectory, onDeleted, onMoved, resetKey }: ImageGridProps) {
  const { onScroll, viewportRef, gridRef, slice, padTop, padBottom } =
    useGridWindow(images.length, resetKey);

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
      <div className="image-tiles" ref={gridRef}>
        {padTop > 0 && <div data-pad="top" style={{ height: padTop, gridColumn: "1 / -1" }} aria-hidden="true" />}
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
        {padBottom > 0 && <div data-pad="bottom" style={{ height: padBottom, gridColumn: "1 / -1" }} aria-hidden="true" />}
      </div>
    </div>
  );
}
