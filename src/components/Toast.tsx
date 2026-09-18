import { useEffect } from "react";

export interface ToastItem {
  id: number;
  message: string;
  tone: "info" | "error";
}

/** 轻量浮动提示：几秒后自动消失，不占布局、不需要手动关闭 */
export function ToastLayer({ toasts, onClose }: { toasts: ToastItem[]; onClose: (id: number) => void }) {
  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col items-end gap-2 pointer-events-none">
      {toasts.map(toast => (
        <Toast key={toast.id} toast={toast} onClose={onClose} />
      ))}
    </div>
  );
}

function Toast({ toast, onClose }: { toast: ToastItem; onClose: (id: number) => void }) {
  useEffect(() => {
    const timer = setTimeout(() => onClose(toast.id), toast.tone === "error" ? 6000 : 3000);
    return () => clearTimeout(timer);
  }, [toast.id, toast.tone, onClose]);

  return (
    <button
      type="button"
      onClick={() => onClose(toast.id)}
      className={`pointer-events-auto max-w-md px-3 py-2 rounded-lg text-sm shadow-lg ring-1 backdrop-blur transition-opacity hover:opacity-80 text-left ${
        toast.tone === "error"
          ? "bg-red-950/90 text-red-100 ring-red-500/40"
          : "bg-slate-800/90 text-slate-100 ring-white/10"
      }`}
    >
      {toast.message}
    </button>
  );
}
