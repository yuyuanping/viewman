import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { MediaKind } from "../types";
import { dedupeKey } from "../scanRoots";
import { setMovePromptFlag } from "../hooks/useDeleteShortcut";

/** 发起一次移动的待办：谁触发移动谁填 onPick，确认与真正的移动都在父层做 */
export interface MovePrompt {
  kind: MediaKind;
  /** 文案里指代待移动条目的名词（"图片" / "视频"） */
  noun: string;
  /** 待移动条数，标题里用 */
  count: number;
  /** 返回 true 表示已经动手（对话框随之关闭），false = 用户反悔或失败（留着好换一个） */
  onPick: (dir: string) => Promise<boolean>;
}

type MoveTargetsDialogProps = MovePrompt & { onClose: () => void };

/**
 * 「移动到…」目标目录列表：常用落点一目了然，点一行就移过去。
 * 清单按媒体类型存在数据库里，行尾 ✕ 删除，「添加目录…」走系统选择器补新的。
 * Esc 用捕获阶段监听：面板/查看器也各自监听 Esc 关自己，得赶在它们前面把事件截下，
 * 不然关个移动对话框会连带把底下整层一起关掉。
 */
export function MoveTargetsDialog({ kind, noun, count, onPick, onClose }: MoveTargetsDialogProps) {
  /** null = 清单还在读取；[] = 读到了但是空的 */
  const [targets, setTargets] = useState<string[] | null>(null);
  /** 正在往这个目录移动（等待父层的 onPick 返回） */
  const [busy, setBusy] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    let disposed = false;
    api.loadMoveTargets(kind)
      .then(list => { if (!disposed) setTargets(list); })
      .catch(() => { if (!disposed) setTargets([]); });
    return () => { disposed = true; };
  }, [kind]);

  useEffect(() => {
    // 挂上全局标记：批量的 Del/M 和查看器里的按键见标志就让位，不许隔着对话框误删误移
    setMovePromptFlag(true);
    return () => setMovePromptFlag(false);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      if (!busy) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, busy]);

  const addDir = async () => {
    if (adding) return;
    setAdding(true);
    let dir: string | null;
    try {
      dir = await open({ directory: true, multiple: false, title: "选择要加入清单的文件夹" });
    } catch {
      setAdding(false);
      return;
    }
    setAdding(false);
    if (!dir) return;
    setTargets(prev => {
      const list = prev ?? [];
      if (list.some(item => dedupeKey(item) === dedupeKey(dir))) return list;
      const next = [...list, dir];
      api.saveMoveTargets(kind, next).catch(() => { /* 同上 */ });
      return next;
    });
  };

  const removeDir = (dir: string) => {
    if (busy) return;
    setTargets(prev => {
      const next = (prev ?? []).filter(item => dedupeKey(item) !== dedupeKey(dir));
      api.saveMoveTargets(kind, next).catch(() => { /* 同上 */ });
      return next;
    });
  };

  const pick = async (dir: string) => {
    if (busy) return;
    setBusy(dir);
    const done = await onPick(dir).catch(() => false);
    setBusy(null);
    if (done) onClose();
  };

  return (
    <div className="image-viewer fixed inset-0 z-50 flex items-center justify-center" role="dialog" aria-modal="true" aria-label="选择移动目标目录">
      <div className="w-[560px] max-w-[92vw] max-h-[80vh] flex flex-col rounded-xl border border-white/10 bg-[#0d1420] shadow-2xl">
        <div className="flex items-center gap-3 px-4 py-3 shrink-0 border-b border-white/10">
          <h2 className="text-sm text-gray-200 font-medium">{`移动 ${count} ${noun}到…`}</h2>
          <span className="toolbar-spacer" />
          <button type="button" className="toolbar-chip" onClick={onClose} title="取消移动（Esc）">
            取消 (Esc)
          </button>
        </div>
        <ul className="flex-1 overflow-y-auto px-2 py-2">
          {targets === null && (
            <li className="text-gray-500 text-sm p-4">正在读取目录清单…</li>
          )}
          {targets !== null && targets.length === 0 && (
            <li className="text-gray-500 text-sm p-4">
              清单还是空的——点下面的「添加目录…」把常用的落点加进来，以后移动就是点一下的事。
            </li>
          )}
          {targets?.map(dir => (
            <li key={dir} className="flex items-center gap-1 rounded-lg hover:bg-white/5">
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => void pick(dir)}
                className="flex-1 min-w-0 text-left px-3 py-2.5 text-[13px] text-gray-200 truncate"
                title={busy === dir ? "正在移动…" : `移动到 ${dir}`}
              >
                {busy === dir ? "移动中… " : ""}{dir}
              </button>
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => removeDir(dir)}
                className="shrink-0 w-7 h-7 grid place-items-center rounded text-gray-500 hover:bg-red-700/70 hover:text-gray-100"
                title="从清单里移除这个目录（不动磁盘上的文件）"
                aria-label={`从清单移除 ${dir}`}
              >
                ✕
              </button>
            </li>
          ))}
        </ul>
        <div className="flex items-center gap-3 px-4 py-3 shrink-0 border-t border-white/10">
          <button type="button" className="toolbar-chip" onClick={() => void addDir()} disabled={adding}>
            {adding ? "选择中…" : "添加目录…"}
          </button>
          <span className="text-xs text-gray-500">清单按图片/视频分开记，存在库里，重装后仍在。</span>
        </div>
      </div>
    </div>
  );
}
