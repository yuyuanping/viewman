import { useCallback, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { Video } from "../types";

/** 文件校验并发数：20 路过桥，19 万条不再串行卡死；进度 200ms 节流一次 */
const CHECK_CONCURRENCY = 20;
const PROGRESS_THROTTLE_MS = 200;

/** 逐个校验文件：不可读的标记为"丢失"，内容实为图片的标记为"假视频"，只标记不动数据库，避免磁盘离线时误判为已删除 */
export function useFileCheck(
  videos: Video[],
  onNotice: (message: string) => void,
  onConverted: () => void,
) {
  const [missingIds, setMissingIds] = useState<Set<string>>(() => new Set());
  const [fakeIds, setFakeIds] = useState<Set<string>>(() => new Set());
  const [checkProgress, setCheckProgress] = useState<{ processed: number; total: number } | null>(null);
  const [converting, setConverting] = useState(false);
  const [convertingShorts, setConvertingShorts] = useState(false);
  // 取消标记：开始一轮时新建，取消按钮/重入时置 true，worker 循环据此提前退出
  const cancelRef = useRef(false);

  const cancelCheck = useCallback(() => {
    cancelRef.current = true;
  }, []);

  const checkFiles = useCallback(async () => {
    if (videos.length === 0 || checkProgress !== null) return;
    cancelRef.current = false;
    const unreadable = new Set<string>();
    const fake = new Set<string>();
    const total = videos.length;
    let processed = 0;
    let lastEmit = 0;
    setCheckProgress({ processed: 0, total });
    const snapshot = videos.slice();
    let next = 0;
    const worker = async () => {
      while (true) {
        if (cancelRef.current) break;
        const i = next++;
        if (i >= snapshot.length) break;
        try {
          const result = await api.checkVideoFile(snapshot[i].id);
          if (cancelRef.current) break;
          if (result.status === "fake_image") fake.add(snapshot[i].id);
          else if (result.status !== "readable") unreadable.add(snapshot[i].id);
        } catch {
          if (!cancelRef.current) unreadable.add(snapshot[i].id);
        }
        processed += 1;
        const now = Date.now();
        if (now - lastEmit >= PROGRESS_THROTTLE_MS || processed >= total) {
          lastEmit = now;
          setCheckProgress({ processed, total });
        }
      }
    };
    try {
      await Promise.all(
        Array.from({ length: Math.min(CHECK_CONCURRENCY, snapshot.length) }, () => worker()),
      );
      if (cancelRef.current) {
        onNotice("文件检查已取消（已标记部分保留）。");
        return;
      }
      setMissingIds(unreadable);
      setFakeIds(fake);
      const parts: string[] = [];
      if (unreadable.size > 0) parts.push(`${unreadable.size} 个文件当前不可读取`);
      if (fake.size > 0) parts.push(`${fake.size} 个实为图片（假视频）`);
      onNotice(parts.length > 0
        ? `文件检查完成：${parts.join("，")}（已标记）。`
        : "文件检查完成：所有文件均正常。");
    } finally {
      setCheckProgress(null);
    }
  }, [videos, onNotice, checkProgress]);

  const convertFakes = useCallback(async () => {
    if (fakeIds.size === 0 || converting) return;
    setConverting(true);
    try {
      const result = await api.convertFakeImages([...fakeIds]);
      setFakeIds(new Set());
      onConverted();
      onNotice(result.errors.length > 0
        ? `已转换 ${result.converted} 个，${result.errors.length} 个未转换：${result.errors[0]}`
        : `已转换 ${result.converted} 个假视频为图片文件。`);
    } catch (err) {
      onNotice("转换失败: " + err);
    } finally {
      setConverting(false);
    }
  }, [fakeIds, converting, onNotice, onConverted]);

  const clearMissing = useCallback(() => setMissingIds(new Set()), []);
  const clearFake = useCallback(() => setFakeIds(new Set()), []);

  /** 静图检测结果快照；null 表示本次会话尚未检测，先用"时长<1秒"的粗筛 */
  const [staticIds, setStaticIds] = useState<string[] | null>(null);
  const [detecting, setDetecting] = useState(false);

  const detectShorts = useCallback(async () => {
    if (detecting) return;
    setDetecting(true);
    try {
      const ids = await api.findStaticVideos();
      setStaticIds(ids);
      onNotice(ids.length > 0
        ? `检测完成：${ids.length} 个静图视频/图片伪装可转图片。`
        : "检测完成：没有发现静图视频或图片伪装文件。");
    } catch (err) {
      onNotice("检测失败: " + err);
    } finally {
      setDetecting(false);
    }
  }, [detecting, onNotice]);

  const shortIds = useMemo(
    () => new Set(staticIds ?? videos.filter(v => v.duration !== null && v.duration < 1).map(v => v.id)),
    [staticIds, videos],
  );
  const shortsDetected = staticIds !== null;
  const clearShorts = useCallback(() => setStaticIds(null), []);

  const convertShorts = useCallback(async () => {
    if (shortIds.size === 0 || convertingShorts) return;
    setConvertingShorts(true);
    try {
      const result = await api.convertShortVideos([...shortIds]);
      setStaticIds(null);
      onConverted();
      onNotice(result.errors.length > 0
        ? `已转换 ${result.converted} 个，${result.errors.length} 个未转换：${result.errors[0]}`
        : `已将 ${result.converted} 个短视频转换为图片。`);
    } catch (err) {
      onNotice("转换失败: " + err);
    } finally {
      setConvertingShorts(false);
    }
  }, [shortIds, convertingShorts, onNotice, onConverted]);

  return {
    missingIds, clearMissing,
    fakeIds, clearFake, convertFakes, converting,
    shortIds, convertShorts, convertingShorts, shortsDetected, detectShorts, detecting, clearShorts,
    checkProgress, checking: checkProgress !== null, checkFiles, cancelCheck,
  };
}
