import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { logger } from "../logger";
import type { Image } from "../types";
import { CARD_TEXT_H } from "../justifiedLayout";
import { copyToClipboard, formatFileSize, formatResolution } from "../utils";
import { ContextMenu, useContextMenu } from "./ContextMenu";
import { MoveTargetsDialog } from "./MoveTargetsDialog";

interface ImageCardProps {
  image: Image;
  /** 对齐布局给的图片区高度：给了就用它，不给走方格默认（aspect-ratio 1/1） */
  imageHeight?: number;
  selectMode?: boolean;
  selected?: boolean;
  duplicate?: boolean;
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

export function ImageCard({ image, imageHeight, selectMode = false, selected = false, duplicate = false, animated = false, onFindSimilar, onToggleSelect, onOpen, onDeleted, onMoved }: ImageCardProps) {
  const [deleting, setDeleting] = useState(false);
  const [moving, setMoving] = useState(false);
  const { menu, openMenu, closeMenu } = useContextMenu<void>();
  const [movePromptOpen, setMovePromptOpen] = useState(false);
  // 卡片级操作失败行内展示，不阻塞
  const [actionError, setActionError] = useState<string | null>(null);

  const openContainingFolder = async () => {
    try {
      await revealItemInDir(image.path);
    } catch (err) {
      logger.warn("ImageCard", "打开所在位置失败:", err);
      setActionError(`打开文件所在位置失败：${String(err)}`);
    }
  };
  const copyFilename = async () => {
    try {
      await copyToClipboard(image.filename);
    } catch (err) {
      logger.warn("ImageCard", "复制文件名失败:", err);
      setActionError(`复制文件名失败：${String(err)}`);
    }
  };

  const handleDelete = async () => {
    setDeleting(true);
    setActionError(null);
    try {
      const report = await api.deleteImages([image.id]);
      if (report.locked.length > 0) {
        setActionError("删除失败：文件被占用，关闭占用它的程序后重试。");
        setDeleting(false);
        return;
      }
      onDeleted(image.id);
    } catch (err) {
      logger.warn("ImageCard", "删除失败:", err);
      setActionError(`删除失败：${String(err)}`);
      setDeleting(false);
    }
  };

  const handleMove = () => {
    closeMenu();
    setMovePromptOpen(true);
  };

  /** 右键菜单：与视频卡片/播放列表同一套，另加模板找相似 */
  const menuItems = [
    { key: "reveal", label: "打开文件所在位置", onSelect: () => { void openContainingFolder(); } },
    { key: "copy", label: "复制文件名", onSelect: () => { void copyFilename(); } },
    ...(onFindSimilar
      ? [{ key: "similar", label: "以此为模板找相似", onSelect: () => onFindSimilar(image) }]
      : []),
    { key: "move", label: moving ? "移动中…" : "移动到…", disabled: moving, onSelect: handleMove },
    { key: "delete", label: deleting ? "删除中…" : "删除到回收站", disabled: deleting, onSelect: () => { void handleDelete(); } },
  ];

  // 没有封面缓存时直接引用原图：靠 loading="lazy" 兜住首屏，生成封面后走缩略图
  const preview = image.thumbnail_path ?? image.path;

  return (
    <article
      className={`image-card${selected ? " media-selected" : ""}${duplicate ? " image-duplicate" : ""}${animated ? " image-animated" : ""}`}
      title={duplicate ? "与库内其他图片内容相同（多余副本，可在工具栏「重复图片分组」里审阅后删除）" : animated ? "多帧动图（GIF/APNG/动态 WebP），勾选后可批量清理" : undefined}
      onContextMenu={(e) => openMenu(e, undefined)}
    >
      <button
        onClick={(e) => {
          // 多选模式里点击切换勾选；没进多选时 Shift 点也按选择算（连选入口，资源管理器式），
          // 普通点击仍是打开查看器
          if (selectMode || e.shiftKey) onToggleSelect?.(e.shiftKey);
          else onOpen(image);
        }}
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
            setMoving(true);
            setActionError(null);
            try {
              const newPath = await api.moveImage(image.id, dir);
              onMoved(image.id, newPath);
              return true;
            } catch (err) {
              logger.warn("ImageCard", "移动失败:", err);
              setActionError(`移动失败：${String(err)}`);
              return false;
            } finally {
              setMoving(false);
            }
          }}
          onClose={() => setMovePromptOpen(false)}
        />
      )}
      {menu && (
        <ContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={closeMenu} label={`${image.filename} 的操作菜单`} />
      )}
      {actionError && (
        <div className="mx-2 mb-2 rounded-lg bg-red-950/90 px-2 py-1.5 text-[11px] leading-4 text-red-200 flex justify-between gap-2">
          <span className="break-all">{actionError}</span>
          <button onClick={() => setActionError(null)} aria-label="关闭错误" className="shrink-0 hover:text-white">✕</button>
        </div>
      )}
    </article>
  );
}
