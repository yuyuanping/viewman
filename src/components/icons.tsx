import type { ReactNode, SVGProps } from "react";

/**
 * 内联 SVG 图标集：lucide 线稿风格（24 视框、stroke=currentColor、2px 圆角描边）。
 * 零依赖、颜色跟随所在文字；取代散落各处的 emoji（📁📄⏮⏭⛶）与字符字形（✕ ✓ ✗ ▼ ▶ ‹ ›）——
 * 字形在系统字体回退下粗细不一、对不齐，SVG 图标跨字体渲染稳定。
 *
 * size 默认 "1em" 跟随所在文字字号（正文里的图标与文字同高），需要固定像素传数字。
 */
export interface IconProps extends SVGProps<SVGSVGElement> {
  size?: number | string;
}

function Icon({ size = "1em", children, ...rest }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      // Tailwind preflight 把 svg 设为 display: block——文字流按钮（toolbar-chip 这类非 flex 容器）
      // 里会独占一行，把文字挤到下一行叠起来。统一压回 inline-block 参与 inline 布局；
      // 基线沉在 x-height 偏下，抬回视觉居中。flex 容器里这两条都无副作用
      style={{ display: "inline-block", verticalAlign: "-0.125em" }}
      {...rest}
    >
      {children}
    </svg>
  );
}

/** 关闭 / 否定 */
export const IconX = (p: IconProps) => (
  <Icon {...p}><path d="M18 6 6 18" /><path d="m6 6 12 12" /></Icon>
);

/** 勾选 / 完成 */
export const IconCheck = (p: IconProps) => (
  <Icon {...p}><path d="M20 6 9 17l-5-5" /></Icon>
);

export const IconChevronDown = (p: IconProps) => (
  <Icon {...p}><path d="m6 9 6 6 6-6" /></Icon>
);

export const IconChevronRight = (p: IconProps) => (
  <Icon {...p}><path d="m9 18 6-6-6-6" /></Icon>
);

export const IconChevronLeft = (p: IconProps) => (
  <Icon {...p}><path d="m15 18-6-6 6-6" /></Icon>
);

export const IconArrowUp = (p: IconProps) => (
  <Icon {...p}><path d="m5 12 7-7 7 7" /><path d="M12 19V5" /></Icon>
);

export const IconArrowDown = (p: IconProps) => (
  <Icon {...p}><path d="M12 5v14" /><path d="m19 12-7 7-7-7" /></Icon>
);

export const IconArrowLeft = (p: IconProps) => (
  <Icon {...p}><path d="m12 19-7-7 7-7" /><path d="M19 12H5" /></Icon>
);

export const IconArrowRight = (p: IconProps) => (
  <Icon {...p}><path d="M5 12h14" /><path d="m12 5 7 7-7 7" /></Icon>
);

export const IconFolder = (p: IconProps) => (
  <Icon {...p}><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" /></Icon>
);

export const IconFile = (p: IconProps) => (
  <Icon {...p}>
    <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
    <path d="M14 2v4a2 2 0 0 0 2 2h4" />
  </Icon>
);

/** 进入全屏（四角向外） */
export const IconMaximize = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8 3H5a2 2 0 0 0-2 2v3" />
    <path d="M21 8V5a2 2 0 0 0-2-2h-3" />
    <path d="M3 16v3a2 2 0 0 0 2 2h3" />
    <path d="M16 21h3a2 2 0 0 0 2-2v-3" />
  </Icon>
);

/** 退出全屏（四角向内） */
export const IconMinimize = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 14h6v6" />
    <path d="M20 10h-6V4" />
    <path d="M14 10l7-7" />
    <path d="M3 21l7-7" />
  </Icon>
);

/** 上一首/上一个 */
export const IconSkipBack = (p: IconProps) => (
  <Icon {...p}>
    <path d="M19 20 9 12l10-8v16Z" fill="currentColor" stroke="none" />
    <path d="M5 19V5" />
  </Icon>
);

/** 下一首/下一个 */
export const IconSkipForward = (p: IconProps) => (
  <Icon {...p}>
    <path d="m5 4 10 8L5 20V4Z" fill="currentColor" stroke="none" />
    <path d="M19 5v14" />
  </Icon>
);
