import { flushSync } from "react-dom";

/**
 * 卡片 → 全屏层（图片查看器/内置播放器）的共享元素过渡（same-document view transition，
 * 对齐 Chrome modern-web-guidance 的 same-document-transitions 指南）。
 *
 * 约定：全屏层一侧的媒体元素在 CSS 里静态持有 view-transition-name: media-hero
 * （[data-morph-hero]，见 styles/index.css）；卡片一侧在过渡期间用内联样式临时持有
 * 同一个名字。同一时刻同一侧只能有一个该名元素，否则浏览器直接跳过整个过渡。
 *
 * 依赖 View Transitions（WebView2 evergreen 自带）；不支持的运行时直接落回普通状态
 * 更新，行为与从前一致。中途被新过渡打断时浏览器自动跳过旧的，回调照常执行，
 * finished.finally 里的名字清理不受影响。
 */
const HERO_NAME = "media-hero";

/** 点卡片打开全屏层：source 是卡片里被点的预览元素（拿不到就退化成普通打开） */
export function morphToOverlay(source: HTMLElement | null | undefined, apply: () => void) {
  if (!source || !document.startViewTransition) {
    apply();
    return;
  }
  source.style.viewTransitionName = HERO_NAME;
  const transition = document.startViewTransition(() => {
    // 旧快照已拍到带名字的卡片；新快照里卡片必须先摘名，给全屏层让出唯一命名位
    source.style.viewTransitionName = "";
    flushSync(apply);
  });
  transition.finished.finally(() => {
    source.style.viewTransitionName = "";
  }).catch(() => { /* 更新回调抛错时 finished 同样reject，名字清理尽力而为 */ });
}

/** 关闭全屏层回网格：按 id 找还在挂载中的卡片（虚拟窗口外的找不到 → 退化成普通关闭，遮罩走根快照交叉淡出） */
export function morphFromOverlay(apply: () => void, sourceId?: string | null) {
  const source = sourceId
    ? document.querySelector<HTMLElement>(`[data-media-id="${CSS.escape(sourceId)}"]`)
    : null;
  if (!source || !document.startViewTransition) {
    apply();
    return;
  }
  // 旧快照里命名的只能有卡片这一个：先摘掉全屏层媒体元素的静态名（内联 none 压过 CSS）
  document.querySelector<HTMLElement>("[data-morph-hero]")
    ?.style.setProperty("view-transition-name", "none");
  source.style.viewTransitionName = HERO_NAME;
  const transition = document.startViewTransition(() => flushSync(apply));
  transition.finished.finally(() => {
    source.style.viewTransitionName = "";
  }).catch(() => { /* 同上 */ });
}
