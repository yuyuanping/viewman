import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { Image } from "../types";
import { formatFileSize, formatResolution } from "../utils";

interface ImageCardProps {
  image: Image;
  selectMode?: boolean;
  selected?: boolean;
  duplicate?: boolean;
  /** 相似图检测命中（pHash）：琥珀色边框提示，区别于红色重复 */
  similar?: boolean;
  onToggleSelect?: (shiftKey: boolean) => void;
  onOpen: (image: Image) => void;
  /** 删除成功后回报 id，父层据此就地剔除，不再重拉整库 */
  onDeleted: (imageId: string) => void;
  onMoved: (imageId: string, newPath: string) => void;
}

export function ImageCard({ image, selectMode = false, selected = false, duplicate = false, similar = false, onToggleSelect, onOpen, onDeleted, onMoved }: ImageCardProps) {
  const [deleting, setDeleting] = useState(false);
  const [moving, setMoving] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  const openContainingFolder = async () => {
    try {
      await revealItemInDir(image.path);
    } catch (err) {
      alert("打开文件所在位置失败: " + err);
    }
  };

  const handleDelete = async () => {
    if (!confirm(`确定要删除 "${image.filename}" 到回收站？`)) return;
    setDeleting(true);
    try {
      const deleted = await api.deleteImages([image.id]);
      if (deleted.length > 0) onDeleted(image.id);
    } catch (err) {
      alert("删除失败: " + err);
      setDeleting(false);
    }
  };

  const handleMove = async () => {
    let dir: string | null;
    try {
      dir = await open({ directory: true, multiple: false, title: "选择目标文件夹" });
    } catch (err) {
      alert("打开文件夹选择器失败: " + err);
      return;
    }
    if (!dir) return;
    if (!confirm(`将 "${image.filename}" 移动到:\n${dir}`)) return;
    setMoving(true);
    try {
      const newPath = await api.moveImage(image.id, dir);
      onMoved(image.id, newPath);
    } catch (err) {
      alert("移动失败: " + err);
    } finally {
      setMoving(false);
    }
  };

  // 没有封面缓存时直接引用原图：靠 loading="lazy" 兜住首屏，生成封面后走缩略图
  const preview = image.thumbnail_path ?? image.path;

  return (
    <article
      className={`image-card${selected ? " media-selected" : ""}${duplicate ? " image-duplicate" : ""}${similar ? " image-similar" : ""}`}
      title={duplicate ? "与库内其他图片内容相同（多余副本，可在工具栏「重复图片分组」里审阅后删除）" : similar ? "与库内其他图片视觉相似（连拍/截图系列）" : undefined}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
      }}
    >
      <button
        onClick={selectMode ? (e) => onToggleSelect?.(e.shiftKey) : () => onOpen(image)}
        disabled={deleting}
        className="block w-full text-left"
        aria-label={selectMode ? (selected ? `取消选择 ${image.filename}` : `选择 ${image.filename}`) : `查看 ${image.filename}`}
      >
        <div className="image-preview">
          {duplicate && <span className="dup-flag">重复副本</span>}
          {selectMode && (
            <span className={`select-check${selected ? " select-checked" : ""}`} aria-hidden="true">
              {selected ? "✓" : ""}
            </span>
          )}
          <img src={convertFileSrc(preview)} alt={image.filename} loading="lazy" className="media-thumbnail" />
          <span className="media-badge bottom-2 right-2">{formatResolution(image.width, image.height)}</span>
        </div>
        <div className="px-2.5 py-2.5">
          <p className="text-[12px] leading-4 line-clamp-1 text-gray-200 break-all" title={image.filename}>
            {image.filename}
          </p>
          <p className="text-[11px] text-gray-400 mt-1.5 tabular-nums">{formatFileSize(image.file_size)}</p>
        </div>
      </button>
      {!selectMode && (
        <button
          onClick={handleDelete}
          disabled={deleting}
          className="media-remove absolute top-1.5 right-1.5 bg-gray-950/80 hover:bg-red-700 text-gray-300 rounded-lg w-7 h-7 grid place-items-center text-sm"
          title="删除到回收站"
          aria-label={`删除 ${image.filename} 到回收站`}
        >
          {deleting ? "…" : "×"}
        </button>
      )}
      {menu && (
        <div
          className="media-context-menu"
          style={{ left: menu.x, top: menu.y }}
          onClick={(e) => e.stopPropagation()}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button type="button" onClick={() => { setMenu(null); void openContainingFolder(); }}>
            打开文件所在位置
          </button>
          <button type="button" disabled={moving} onClick={() => { setMenu(null); void handleMove(); }}>
            {moving ? "移动中…" : "移动到…"}
          </button>
        </div>
      )}
    </article>
  );
}
