import { useCallback, useState } from "react";
import type { ToastItem } from "../components/Toast";

export type Notify = (message: string, tone?: ToastItem["tone"]) => void;

/** 浮动提示队列：notify 追加，几秒后由 Toast 组件自动消失 */
export function useToasts() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);

  const notify = useCallback<Notify>((message, tone = "info") => {
    const id = Date.now() + Math.random();
    setToasts(prev => [...prev, { id, message, tone }]);
  }, []);

  const dismiss = useCallback((id: number) => {
    setToasts(prev => prev.filter(t => t.id !== id));
  }, []);

  return { toasts, notify, dismiss };
}
