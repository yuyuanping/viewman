import { useEffect } from "react";

/** 移动目标目录对话框开着时在 body 上打的标记：所有全局快捷键见它就让位 */
const MOVE_PROMPT_FLAG = "movePromptOpen";

/** 「移动到…」目录列表对话框是否开着（卡片级对话框不经过各页面的 state，只能看全局标记） */
export function isMovePromptOpen(): boolean {
  return document.body.dataset[MOVE_PROMPT_FLAG] === "1";
}

/** 对话框挂载/卸载时由 MoveTargetsDialog 调用 */
export function setMovePromptFlag(on: boolean): void {
  if (on) document.body.dataset[MOVE_PROMPT_FLAG] = "1";
  else delete document.body.dataset[MOVE_PROMPT_FLAG];
}

/** 焦点在输入控件里时，字符键/删除键都属于编辑行为（搜索框里打字删字符），不能顺带触发批量操作 */
function editableTarget(target: { tagName?: string; isContentEditable?: boolean } | null): boolean {
  const tag = target?.tagName ?? "";
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target?.isContentEditable === true;
}

/** 这次按键该不该触发「删除所选」 */
export function deletesOnKey(key: string, target: { tagName?: string; isContentEditable?: boolean } | null): boolean {
  return key === "Delete" && !editableTarget(target);
}

/** 这次按键该不该触发「移动所选」 */
export function movesOnKey(key: string, target: { tagName?: string; isContentEditable?: boolean } | null): boolean {
  return (key === "m" || key === "M") && !editableTarget(target);
}

/** 数字键直达移动的谓词：1-9 且不在输入控件里 */
export function movesOnNumberKey(key: string, target: { tagName?: string; isContentEditable?: boolean } | null): boolean {
  return key >= "1" && key <= "9" && !editableTarget(target);
}

function useKeyShortcut(fires: (key: string, target: { tagName?: string; isContentEditable?: boolean } | null) => boolean, onFire: (key: string) => void, enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (isMovePromptOpen()) return;
      if (!fires(e.key, e.target as HTMLElement | null)) return;
      e.preventDefault();
      onFire(e.key);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fires, onFire, enabled]);
}

/** Del 键 = 「删除所选」；什么时候不算由调用方用 enabled 决定（查看器盖在上面等） */
export function useDeleteShortcut(onDelete: () => void, enabled: boolean): void {
  useKeyShortcut(deletesOnKey, onDelete, enabled);
}

/** M 键 = 「移动所选」：与 Del 同一套门禁，弹目录选择器后批量移动 */
export function useMoveShortcut(onMove: () => void, enabled: boolean): void {
  useKeyShortcut(movesOnKey, onMove, enabled);
}

/** 数字键直达移动：1/2/3… = 目标清单第 1/2/3… 项，一次按键即批量移动（弹清单那步全省了） */
export function useMoveNumberShortcut(
  targets: string[] | null | undefined,
  onMoveTo: (dir: string) => void,
  enabled: boolean,
): void {
  useKeyShortcut(movesOnNumberKey, (key) => {
    const dir = targets?.[Number(key) - 1];
    if (dir) onMoveTo(dir);
  }, enabled);
}
