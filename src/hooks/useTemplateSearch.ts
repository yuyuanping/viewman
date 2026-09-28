import { useCallback, useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { TemplateMatch, TemplateProgressPayload } from "../api";
import type { Image } from "../types";
import type { Notify } from "./useToasts";

interface TemplateSearchOptions {
  /** 按 id 取图：命中的元数据现查现用 */
  fetchImagesByIds: (ids: string[]) => Promise<Image[]>;
  /** 库内容代次：删除/移动/扫描之后 bump，命中清单据此裁掉退库的条目 */
  libraryVersion: number;
  setSelectMode: (on: boolean) => void;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  notify: Notify;
}

/**
 * 命中过滤的默认宽容度（求和口径：两枚指纹的距离之和）。
 * 实测（18 万张库、以一张评论区文字截图为模板）：真正的"同版式"近邻落在 28–34，
 * 撞车洪峰从 40 出头才起（≤36 只有 34 条，≤40 就有 202 条，≤48 有 4564 条）——
 * 默认 32 卡在平台区和洪峰之间；滑杆只在前端过滤，改了不用重跑。
 */
export const TEMPLATE_THRESHOLD_DEFAULT = 32;
export const TEMPLATE_THRESHOLD_MAX = 48;

const NO_INDEX = new Map<string, Image>();
const NO_MATCHES: TemplateMatch[] = [];

/**
 * 模板匹配（以图搜图）：选定一张图当模板，后端比一遍全库指纹，命中按距离升序。
 * 与相似检测不同，这里没有分组、没有自动勾选——结果是一份排名清单，勾删由人逐张决定。
 */
export function useTemplateSearch({ fetchImagesByIds, libraryVersion, setSelectMode, setSelectedIds, notify }: TemplateSearchOptions) {
  const [template, setTemplate] = useState<Image | null>(null);
  const [matches, setMatches] = useState<TemplateMatch[]>(NO_MATCHES);
  const [skipped, setSkipped] = useState(0);
  const [threshold, setThreshold] = useState(TEMPLATE_THRESHOLD_DEFAULT);
  const [searching, setSearching] = useState(false);
  const [progress, setProgress] = useState<{ processed: number; total: number } | null>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  /** 命中条目的元数据（缩略图/尺寸/体积），按 id 现查 */
  const [imageById, setImageById] = useState<Map<string, Image>>(NO_INDEX);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<TemplateProgressPayload>("template-progress", (event) => {
      const { processed, total } = event.payload;
      setProgress(total === 0 ? null : { processed, total });
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 收不到只是看不到补算进度，结束时仍会拿到完整结果 */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const search = useCallback(async (tpl: Image) => {
    setTemplate(tpl);
    setMatches(NO_MATCHES);
    setSkipped(0);
    setPanelOpen(true);
    setSearching(true);
    setProgress(null);
    try {
      const result = await api.findImagesLikeTemplate(tpl.id);
      setMatches(result.matches);
      setSkipped(result.skipped);
      // 元数据现查：命中可能散在全库，不为它建整库索引。
      // 模板不覆盖——传进来的 tpl 本来就是全量元数据，再覆盖只会白查一趟
      const rows = await fetchImagesByIds(result.matches.map(m => m.id));
      setImageById(new Map(rows.map(image => [image.id, image])));
      if (result.matches.length === 0) {
        notify(result.skipped > 0
          ? `没有找到相似的图片（另有 ${result.skipped} 张解不出指纹，未参与比对）。`
          : "没有找到相似的图片。");
      }
    } catch (e) {
      setPanelOpen(false);
      notify(`模板匹配失败：${String(e)}`, "error");
    } finally {
      setSearching(false);
      setProgress(null);
    }
  }, [fetchImagesByIds, notify]);

  const clear = useCallback(() => {
    setTemplate(null);
    setMatches(NO_MATCHES);
    setSkipped(0);
    setImageById(NO_INDEX);
    setPanelOpen(false);
  }, []);

  /** 剪枝只跟库代次走：matches/template 经 ref 读取，不进依赖——
   *  之前挂在它们身上，搜索落定的瞬间一枚"过期单条查询"先回来，
   *  alive 里只有模板自己，把刚到的命中清单整个剪成了空 */
  const matchesRef = useRef(matches);
  const templateRef = useRef(template);
  useEffect(() => { matchesRef.current = matches; }, [matches]);
  useEffect(() => { templateRef.current = template; }, [template]);

  useEffect(() => {
    const cur = matchesRef.current;
    const curTpl = templateRef.current;
    if (cur.length === 0 && !curTpl) return;
    let disposed = false;
    const wanted = [...new Set(
      [curTpl?.id, ...cur.map(m => m.id)].filter((id): id is string => Boolean(id)),
    )];
    fetchImagesByIds(wanted).then(rows => {
      if (disposed) return;
      const alive = new Set(rows.map(r => r.id));
      setMatches(prev => {
        const next = prev.filter(m => alive.has(m.id));
        return next.length === prev.length ? prev : next;
      });
      setImageById(new Map(rows.map(image => [image.id, image])));
      setTemplate(prev => (prev && !alive.has(prev.id) ? null : prev));
    }).catch(() => { /* 查不动就先留着旧清单 */ });
    return () => { disposed = true; };
  }, [libraryVersion, fetchImagesByIds]);

  /** 勾选翻转给定 id 清单：on=true 全勾上，on=false 全取消（面板的"全选命中"用） */
  const selectIds = useCallback((ids: string[], on: boolean) => {
    if (on) setSelectMode(true);
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const id of ids) {
        if (on) next.add(id); else next.delete(id);
      }
      return next;
    });
  }, [setSelectMode, setSelectedIds]);

  return {
    template,
    matches,
    imageById,
    skipped,
    threshold,
    setThreshold,
    searching,
    progress,
    panelOpen,
    setPanelOpen,
    search,
    clear,
    selectIds,
  };
}
