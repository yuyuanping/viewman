/** 分级日志：dev 输出到控制台，prod 只收 warn/error；Tauri 命令错误统一走 warn+notify 分级。
 * 替代散落的 console.warn / 空 catch，搜索时按标签过滤。
 */
export type LogLevel = "debug" | "info" | "warn" | "error";

const isDev = typeof import.meta !== "undefined" && (import.meta as { env?: { DEV?: boolean } }).env?.DEV;

function emit(level: LogLevel, tag: string, ...args: unknown[]) {
  if (!isDev && (level === "debug" || level === "info")) return;
  const fn = level === "error" ? console.error : level === "warn" ? console.warn : console.log;
  fn(`[${tag}]`, ...args);
}

export const logger = {
  debug: (tag: string, ...args: unknown[]) => emit("debug", tag, ...args),
  info: (tag: string, ...args: unknown[]) => emit("info", tag, ...args),
  warn: (tag: string, ...args: unknown[]) => emit("warn", tag, ...args),
  error: (tag: string, ...args: unknown[]) => emit("error", tag, ...args),
};
