import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/** 订阅一个 Tauri 事件：自动处理「组件已卸载但订阅还在路上」的竞态并负责清理。
 *  handler/onError 存在 ref 里，订阅只建立一次，回调始终用最新版本。 */
export function useTauriEvent<T>(event: string | null | undefined, handler: (payload: T) => void, onError?: (e: unknown) => void) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;
  const errorRef = useRef(onError);
  errorRef.current = onError;

  useEffect(() => {
    if (!event) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<T>(event, (received) => handlerRef.current(received.payload))
      .then((fn) => { if (disposed) fn(); else unlisten = fn; })
      .catch((e) => { errorRef.current?.(e); });
    return () => { disposed = true; unlisten?.(); };
  }, [event]);
}
