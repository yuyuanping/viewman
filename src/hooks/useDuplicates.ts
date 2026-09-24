import { useCallback, useMemo, useState } from "react";
import type { Notify } from "./useToasts";

interface DuplicateOptions {
  detect: () => Promise<string[][]>;
  remove: (id: string) => Promise<void>;
  /** 提示文案里的媒体名称 */
  unit: string;
}

/** 重复条目：按内容指纹分组检测，标记每组之外的多余副本并可一键移入回收站 */
export function useDuplicates(
  reload: () => Promise<void>,
  notify: Notify,
  { detect: detectDuplicates, remove, unit }: DuplicateOptions,
) {
  const [groups, setGroups] = useState<string[][]>([]);
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [deleting, setDeleting] = useState(false);

  // 每组第一个是保留的原件（后端按添加时间升序排好），其余为副本
  const extraIds = useMemo(() => new Set(groups.flatMap(g => g.slice(1))), [groups]);

  const detect = useCallback(async () => {
    setDetecting(true);
    try {
      const found = await detectDuplicates();
      setGroups(found);
      setDetected(true);
      const extras = found.reduce((n, g) => n + g.length - 1, 0);
      if (found.length === 0) {
        notify(`没有发现内容相同的重复${unit}。`);
        setDetected(false);
      } else {
        notify(`发现 ${found.length} 组重复${unit}，共 ${extras} 个多余副本。`);
      }
    } catch (e) {
      notify(`检测重复失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
    }
  }, [detectDuplicates, notify, unit]);

  const deleteExtras = useCallback(async () => {
    const ids = [...extraIds];
    if (ids.length === 0) return;
    if (!confirm(`每个重复组保留最早添加的一个，将其余 ${ids.length} 个副本移入回收站？`)) return;
    setDeleting(true);
    let ok = 0;
    let failed = 0;
    for (const id of ids) {
      try {
        await remove(id);
        ok += 1;
      } catch {
        failed += 1;
      }
    }
    await reload();
    setGroups([]);
    setDetected(false);
    notify(failed > 0 ? `已删除 ${ok} 个重复副本，${failed} 个失败（可能被占用）` : `已删除 ${ok} 个重复副本到回收站。`);
    setDeleting(false);
  }, [extraIds, reload, notify, remove, unit]);

  const clear = useCallback(() => {
    setGroups([]);
    setDetected(false);
  }, []);

  return {
    duplicateGroupCount: groups.length,
    duplicateExtrasCount: extraIds.size,
    duplicateIds: extraIds,
    duplicatesDetected: detected,
    detecting,
    detect,
    deleting,
    deleteExtras,
    clear,
  };
}
