import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { MediaKind } from "../types";
import { dedupeKey } from "../scanRoots";
import { setMovePromptFlag } from "../hooks/useDeleteShortcut";
import { useMoveTargets } from "../hooks/useMoveTargets";

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
 * 清单按媒体类型存在数据库里（与数字键直达共享 useMoveTargets 缓存，增删即时同步），
 * 行尾 ✕ 删除，「添加目录…」走系统选择器补新的。
 * Esc 用捕获阶段监听：面板/查看器也各自监听 Esc 关自己，得赶在它们前面把事件截下，
 * 不然关个移动对话框会连带把底下整层一起关掉。
 */
export function MoveTargetsDialog({ kind, noun, count, onPick, onClose }: MoveTargetsDialogProps) {
  const { targets, setTargets } = useMoveTargets(kind);
  /** 正在往这个目录移动（等待父层的 onPick 返回） */
  const [busy, setBusy] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  /** 键盘/鼠标当前选中的行：默认第一项，M 确认即移 */
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement | null>(null);
  const cancelRef = useRef<HTMLButtonElement | null>(null);

  const pick = useCallback(async (dir: string) => {
    if (busy) return;
    setBusy(dir);
    const done = await onPick(dir).catch(() => false);
    setBusy(null);
    if (done) onClose();
  }, [busy, onPick, onClose]);

  // 行数与选中行收敛：删行后 active 不能悬空（与待移动条数 prop 同名，叫 rowCount）
  const rowCount = targets?.length ?? 0;
  const clamped = rowCount === 0 ? 0 : Math.min(active, rowCount - 1);

  useEffect(() => {
    // 挂上全局标记：批量的 Del/M 和查看器里的按键见标志就让位，不许隔着对话框误删误移
    setMovePromptFlag(true);
    return () => setMovePromptFlag(false);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        if (!busy) onClose();
        return;
      }
      // 上下切行、M 确认移动。不用回车：焦点落在背后按钮上时回车会被吞（表现为按了没反应，
      // 实际是背后按钮又被点了一次），M 没有这个原生行为，焦点在哪儿都生效。
      // 忽略按住不放的 repeat：发起移动的那一下 M 还按着时，不能替用户直接移进第一项目录
      if (busy || rowCount === 0) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        setActive((prev) => {
          const len = listRef.current?.querySelectorAll("[data-row]").length ?? rowCount;
          if (len === 0) return 0;
          return e.key === "ArrowDown" ? (prev + 1) % len : (prev - 1 + len) % len;
        });
        return;
      }
      // 数字键直达：1/2/3… = 清单第 1/2/3… 行，一次按键即移动，省去「切行 + M」两步。
      // 清单前几项是高频落点（把常用目录放前 3 位），数字键正好对应。同 M 一样忽略 repeat。
      if (e.key >= "1" && e.key <= "9" && !e.repeat) {
        e.preventDefault();
        e.stopPropagation();
        const dir = targets?.[Number(e.key) - 1];
        if (dir) void pick(dir);
      }
      if ((e.key === "m" || e.key === "M") && !e.repeat) {
        e.preventDefault();
        e.stopPropagation();
        const dir = targets?.[clamped];
        if (dir) void pick(dir);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, busy, rowCount, clamped, targets, pick]);

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

  // 选中行越界收敛 effect：删行后 active 不能悬空（clamped 上面已算好）
  useEffect(() => {
    if (clamped !== active) setActive(clamped);
  }, [clamped, active]);

  // 选中行滚动进视野 + 焦点收进对话框：模态层里焦点落在背后按钮上容易误操作，
  // 收进来之后 ↑↓/M 顺着清单走，Tab 也落在对话框内
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-row="${clamped}"]`);
    if (el) {
      el.focus({ preventScroll: true });
      el.scrollIntoView({ block: "nearest" });
    } else {
      cancelRef.current?.focus({ preventScroll: true });
    }
  }, [clamped, targets]);

  return (
    <div className="image-viewer fixed inset-0 z-50 flex items-center justify-center" role="dialog" aria-modal="true" aria-label="选择移动目标目录">
      <div className="w-[560px] max-w-[92vw] max-h-[80vh] flex flex-col rounded-xl border border-white/10 bg-[#0d1420] shadow-2xl">
        <div className="flex items-center gap-3 px-4 py-3 shrink-0 border-b border-white/10">
          <h2 className="text-sm text-gray-200 font-medium">{`移动 ${count} ${noun}到…`}</h2>
          {targets && targets.length > 0 && (
            <span className="text-xs text-gray-500 shrink-0">数字键 1-9 直达对应目录</span>
          )}
          <span className="toolbar-spacer" />
          <button ref={cancelRef} type="button" className="toolbar-chip" onClick={onClose} title="取消移动（Esc）">
            取消 (Esc)
          </button>
        </div>
        <ul ref={listRef} className="flex-1 overflow-y-auto px-2 py-2">
          {targets === null && (
            <li className="text-gray-500 text-sm p-4">正在读取目录清单…</li>
          )}
          {targets !== null && targets.length === 0 && (
            <li className="text-gray-500 text-sm p-4">
              清单还是空的——点下面的「添加目录…」把常用的落点加进来，以后移动就是点一下的事。
            </li>
          )}
          {targets?.map((dir, i) => (
            <li
              key={dir}
              className={`flex items-center gap-1 rounded-lg ${i === clamped ? "bg-white/10 ring-1 ring-blue-500/50" : "hover:bg-white/5"}`}
            >
              <button
                type="button"
                data-row={i}
                disabled={busy !== null}
                onClick={() => void pick(dir)}
                onMouseEnter={() => setActive(i)}
                onFocus={() => setActive(i)}
                className="flex-1 min-w-0 text-left px-3 py-2.5 text-[13px] text-gray-200 truncate"
                title={busy === dir ? "正在移动…" : `移动到 ${dir}（M）`}
              >
                {i < 9 && <span className="inline-block w-4 mr-2 text-gray-500 tabular-nums">{i + 1}</span>}
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
