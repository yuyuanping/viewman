import type { ReactNode } from "react";

/** 空状态插画：与空库插画同风格（圆角方框 + 线性图标），但多一层设计感——
 *  双层卡片错位错色、图标带发光晕、标题/副文案分两级字重，比原"单框两行字"耐看 */
export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  /** 线性图标（24×24 viewBox，stroke 版） */
  icon: ReactNode;
  title: string;
  hint: string;
  action?: ReactNode;
}) {
  return (
    <div className="flex-1 flex items-center justify-center text-gray-500">
      <div className="text-center max-w-xs">
        <div className="relative mx-auto mb-6 w-20 h-20 grid place-items-center" aria-hidden="true">
          {/* 两层错位卡片垫底：后层旋转、前层正位，做出"堆叠"的层次 */}
          <div className="absolute inset-0 rounded-2xl border border-white/5 bg-gray-800/40 -rotate-6" />
          <div className="absolute inset-0 rounded-2xl border border-white/10 bg-gray-800/80 grid place-items-center shadow-[0_12px_32px_-8px_#00000099]">
            {/* 图标后垫一层强调色光晕，比纯色图标更"发光" */}
            <div className="relative grid place-items-center">
              <div className="absolute w-10 h-10 rounded-full bg-blue-500/20 blur-md" />
              <div className="relative text-blue-400">{icon}</div>
            </div>
          </div>
        </div>
        <p className="text-base font-medium text-gray-100 tracking-tight">{title}</p>
        <p className="text-[13px] mt-1.5 text-gray-500 leading-5">{hint}</p>
        {action && <div className="mt-5">{action}</div>}
      </div>
    </div>
  );
}
