import { useCallback, useMemo, useState } from "react";
import { api } from "../api";
import type { Video } from "../types";

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

  const checkFiles = useCallback(async () => {
    if (videos.length === 0) return;
    const unreadable = new Set<string>();
    const fake = new Set<string>();
    setCheckProgress({ processed: 0, total: videos.length });
    try {
      for (let i = 0; i < videos.length; i++) {
        try {
          const result = await api.checkVideoFile(videos[i].id);
          if (result.status === "fake_image") fake.add(videos[i].id);
          else if (result.status !== "readable") unreadable.add(videos[i].id);
        } catch {
          unreadable.add(videos[i].id);
        }
        setCheckProgress({ processed: i + 1, total: videos.length });
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
  }, [videos, onNotice]);

  const convertFakes = useCallback(async () => {
    if (fakeIds.size === 0 || converting) return;
    if (!confirm(
      `将 ${fakeIds.size} 个假视频按真实内容另存为图片文件（.jpg/.png 等），源文件移入回收站并从库中移除。确定？`,
    )) return;
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
    if (!confirm(
      `将对 ${shortIds.size} 个静图视频/图片伪装文件转换为图片（静图视频抽首帧，伪装图片按真实格式另存），在原目录生成同名 .jpg；原文件移入回收站并从库中移除。确定？`,
    )) return;
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
    checkProgress, checking: checkProgress !== null, checkFiles,
  };
}
