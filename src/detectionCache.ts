/**
 * 检测结果落盘缓存的纯逻辑。
 *
 * 三趟检测（重复图 / 相似图 / 重复视频）都是"点开一次要等几分钟"的活，后端把每趟的结论
 * 落一份 JSON，重启后先按它把组恢复出来看。但那份名单是照着当时的库算的，拿来用之前
 * 得先照现在的库修一遍，并且把"它未必还准"说清楚——这里就放这两件事的纯函数。
 */

/**
 * 恢复出来的结果算不算过期：只有"库里又添过条目"才会让旧结果漏判（新条目从没进过比对），
 * 才值得提示重跑。
 *
 * 少掉的条目不算：liveGroups / pruneGroupsToLive 会当场把它们从组里裁掉，显示出来的
 * 分组本身并没有错，再挂一句"过期了"只是噪声。
 *
 * 只按条数判断，所以"删十张又添十张"这种等量增删看不出来——要覆盖它得把整份 id 清单
 * 或库的时间戳也存进缓存，代价是缓存跟着库一起长。
 */
export function restoredResultStale(restoredCount: number | null, libraryCount: number): boolean {
  if (restoredCount === null) return false;
  return libraryCount > restoredCount;
}

/** 恢复出来的结果过期时要说的一句话：面板标题、工具栏、通知共用同一句 */
export const STALE_RESULT_NOTE = "结果是上次检测存下的，之后库里又添了条目，建议重跑";

/**
 * 裁掉已不在库的组员；少到不足 2 个的组不再算一组。
 * 上一趟的名单里还挂着这期间被删/被移走的条目，不裁就会把"多少组、多少副本"报多。
 */
export function pruneGroupsToLive(groups: string[][], has: (id: string) => boolean): string[][] {
  return groups.map(group => group.filter(has)).filter(group => group.length > 1);
}
