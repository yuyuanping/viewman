import { useEffect, useState } from "react";
import type { MediaKind } from "../types";
import { api } from "../api";

/**
 * 「移动到…」目标目录清单的共享缓存：对话框和数字键直达（勾选后按 1-9 直接移动）
 * 都要用同一份清单，各自去读库会不同步。加载失败返回 []，不阻塞交互。
 */
export function useMoveTargets(kind: MediaKind) {
  const [targets, setTargets] = useState<string[] | null>(null);
  useEffect(() => {
    let disposed = false;
    api.loadMoveTargets(kind)
      .then(list => { if (!disposed) setTargets(list); })
      .catch(() => { if (!disposed) setTargets([]); });
    return () => { disposed = true; };
  }, [kind]);
  return { targets, setTargets };
}
