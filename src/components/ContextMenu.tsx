import { useCallback, useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { logger } from "../logger";
import { copyToClipboard } from "../utils";

/** 右键菜单项：三处卡片/列表共用同一套文案与顺序（打开所在位置 / 复制文件名 / …扩展… / 移动到… / 删除到回收站） */
export interface MenuItem {
  key: string;
  label: string;
  disabled?: boolean;
  onSelect: () => void;
}

/** 菜单预估尺寸：用于视口钳制，避免靠近右/下边缘时溢出可视区 */
const MENU_W = 220;
const ROW_H = 36;

/**
 * 右键菜单状态：定位 + 点空处/Esc 关闭。
 * 各调用方只管拼 items（顺序统一），定位钳制与关闭逻辑收敛在此。
 */
export function useContextMenu<T>() {
  const [menu, setMenu] = useState<{ x: number; y: number; target: T } | null>(null);
  const openMenu = useCallback((e: React.MouseEvent, target: T) => {
    e.preventDefault();
    setMenu({ x: e.clientX, y: e.clientY, target });
  }, []);
  const closeMenu = useCallback(() => setMenu(null), []);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    // Esc 只关菜单不继续冒泡：面板/查看器也监听 Esc 关自己，不能连带关掉底下整层
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
      }
    };
    window.addEventListener("click", close);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [menu]);

  return { menu, openMenu, closeMenu };
}

/**
 * 菜单里的文件动作（打开所在位置 / 复制文件名）：面板与查看器共用，
 * 失败走 onError（父层转 toast），与卡片的行内错误展示语义一致。
 */
export function useFileMenuActions(onError?: (message: string) => void) {
  const reveal = useCallback((path: string) => {
    void revealItemInDir(path).catch((err) => {
      logger.warn("ContextMenu", "打开所在位置失败:", err);
      onError?.(`打开文件所在位置失败：${String(err)}`);
    });
  }, [onError]);
  const copyName = useCallback((filename: string) => {
    void copyToClipboard(filename).catch((err) => {
      logger.warn("ContextMenu", "复制文件名失败:", err);
      onError?.(`复制文件名失败：${String(err)}`);
    });
  }, [onError]);
  return { reveal, copyName };
}

export function ContextMenu({ x, y, items, onClose, label }: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
  label: string;
}) {
  const estH = items.length * ROW_H + 16;
  const left = Math.max(8, Math.min(x, window.innerWidth - MENU_W - 8));
  const top = Math.max(8, Math.min(y, window.innerHeight - estH - 8));
  return (
    <div
      className="media-context-menu"
      role="menu"
      aria-label={label}
      style={{ left, top, minWidth: MENU_W }}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map(item => (
        <button
          key={item.key}
          type="button"
          role="menuitem"
          disabled={item.disabled}
          onClick={() => { onClose(); item.onSelect(); }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
