import { useCallback, useEffect, useRef, useState } from "react";

// 对指定元素请求/退出浏览器全屏，实时跟随 fullscreenchange 同步状态
export function useFullscreen<T extends HTMLElement>() {
  const targetRef = useRef<T>(null);
  const [isFullscreen, setIsFullscreen] = useState(false);

  useEffect(() => {
    const onChange = () => setIsFullscreen(document.fullscreenElement != null);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  const toggleFullscreen = useCallback(() => {
    if (document.fullscreenElement) {
      void document.exitFullscreen().catch(() => undefined);
    } else {
      void targetRef.current?.requestFullscreen().catch(() => undefined);
    }
  }, []);

  return { targetRef, isFullscreen, toggleFullscreen };
}
