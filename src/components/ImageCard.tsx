import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import type { Image } from "../types";
import { CARD_TEXT_H } from "../justifiedLayout";
import { copyToClipboard, formatFileSize, formatResolution } from "../utils";
import { MoveTargetsDialog } from "./MoveTargetsDialog";

interface ImageCardProps {
  image: Image;
  /** 对齐布局给的图片区高度：给了就用它，不给走方格默认（aspect-ratio 1/1） */
  imageHeight?: number;
  selectMode?: boolean;
  selected?: boolean;
  duplicate?: boolean;
  /** 相似图检测命中（pHash）：琥珀色边框提示，区别于红色重复 */
  similar?: boolean;
  /** 动图检测命中（多帧）：紫色边框 + 左上角「动图」标记 */
  animated?: boolean;
  /** 以这张图为模板找库内相似图；不给就不显示右键菜单项 */
  onFindSimilar?: (image: Image) => void;
  onToggleSelect?: (shiftKey: boolean) => void;
  onOpen: (image: Image) => void;
  /** 删除成功后回报 id，父层据此就地剔除，不再重拉整库 */
  onDeleted: (imageId: string) => void;
  onMoved: (imageId: string, newPath: string) => void;
}

export function ImageCard({ image, imageHeight, selectMode = false, selected = false, duplicate = false, similar = false, animated = false, onFindSimilar, onToggleSelect, onOpen, onDeleted, onMoved }: ImageCardProps) {
  const [deleting, setDeleting] = useState(false);
  const [moving, setMoving] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [movePromptOpen, setMovePromptOpen] = useState(false);

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
  const copyFilename = async () => {
    try {
      await copyToClipboard(image.filename);
    } catch (err) {
      alert("复制文件名失败: " + err);
    }
  };

  const handleDelete = async () => {
    if (!confirm(`确定要删除 "${image.filename}" 到回收站？`)) return;
    setDeleting(true);
    try {
      const deleted = await api.deleteImages([image.id]);
      if (deleted.length > 0) {
        onDeleted(image.id);
      } else {
        alert("删除失败：文件可能被占用或已不在库中。");
        setDeleting(false);
      }
    } catch (err) {
      alert("删除失败: " + err);
      setDeleting(false);
    }
  };

  const handleMove = () => {
    setMenu(null);
    setMovePromptOpen(true);
  };

  // 没有封面缓存时直接引用原图：靠 loading="lazy" 兜住首屏，生成封面后走缩略图
  const preview = image.thumbnail_path ?? image.path;

  return (
    <article
      className={`image-card${selected ? " media-selected" : ""}${duplicate ? " image-duplicate" : ""}${similar ? " image-similar" : ""}${animated ? " image-animated" : ""}`}
      title={duplicate ? "与库内其他图片内容相同（多余副本，可在工具栏「重复图片分组」里审阅后删除）" : similar ? "与库内其他图片视觉相似（连拍/截图系列）" : animated ? "多帧动图（GIF/APNG/动态 WebP），勾选后可批量清理" : undefined}
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
        <div className="image-preview" style={imageHeight !== undefined ? { height: imageHeight, width: "100%" } : undefined}>
          {duplicate && <span className="dup-flag">重复副本</span>}
          {animated && <span className="anim-flag">动图</span>}
          {selectMode && (
            <span className={`select-check${selected ? " select-checked" : ""}`} aria-hidden="true">
              {selected ? "✓" : ""}
            </span>
          )}
          <img src={convertFileSrc(preview)} alt={image.filename} loading="lazy" className="media-thumbnail" />
          <span className="media-badge bottom-2 right-2">{formatResolution(image.width, image.height)}</span>
        </div>
        <div className="px-2.5 pt-2 pb-2 overflow-hidden" style={{ height: CARD_TEXT_H }}>
          <p className="text-[12px] leading-4 line-clamp-1 text-gray-200 break-all" title={image.filename}>
            {image.filename}
          </p>
          <p className="text-[11px] leading-4 text-gray-400 mt-1 tabular-nums">{formatFileSize(image.file_size)}</p>
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
      {movePromptOpen && (
        <MoveTargetsDialog
          kind="image"
          noun="图片"
          count={1}
          onPick={async (dir) => {
            if (!confirm(`将 "${image.filename}" 移动到:\n${dir}`)) return false;
            setMoving(true);
            try {
              const newPath = await api.moveImage(image.id, dir);
              onMoved(image.id, newPath);
              return true;
            } catch (err) {
              alert("移动失败: " + err);
              return false;
            } finally {
              setMoving(false);
            }
          }}
          onClose={() => setMovePromptOpen(false)}
        />
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
          <button type="button" onClick={() => { setMenu(null); void copyFilename(); }}>
            复制文件名
          </button>
          {onFindSimilar && (
            <button type="button" onClick={() => { setMenu(null); onFindSimilar(image); }}>
              以此为模板找相似
            </button>
          )}
          <button type="button" disabled={moving} onClick={() => { setMenu(null); void handleMove(); }}>
            {moving ? "移动中…" : "移动到…"}
          </button>
        </div>
      )}
    </article>
  );
}
