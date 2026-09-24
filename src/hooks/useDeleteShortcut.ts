import { useEffect } from "react";

/**
 * 这次按键该不该触发「删除所选」。
 * 焦点在输入控件里时 Del 属于编辑行为（搜索框里删字符），不能顺带批量删条目。
 */
export function deletesOnKey(key: string, target: { tagName?: string; isContentEditable?: boolean } | null): boolean {
  if (key !== "Delete") return false;
  const tag = target?.tagName ?? "";
  if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable === true) return false;
  return true;
}

/** Del 键 = 「删除所选」；什么时候不算由调用方用 enabled 决定（查看器盖在上面等） */
export function useDeleteShortcut(onDelete: () => void, enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (!deletesOnKey(e.key, e.target as HTMLElement | null)) return;
      e.preventDefault();
      onDelete();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDelete, enabled]);
}
