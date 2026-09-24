/** 按 id 合并一批条目：已有的整条覆盖，新的追加到末尾；没有增量时原数组照旧返回。
 *  扫描只回传本轮动过的几条，前端就不必为了同步清单再整库重拉一次几十 MB。 */
export function mergeById<T extends { id: string }>(current: T[], incoming: T[]): T[] {
  if (incoming.length === 0) return current;
  const indexOf = new Map(current.map((item, at) => [item.id, at]));
  const merged = current.slice();
  const appended: T[] = [];
  for (const item of incoming) {
    const at = indexOf.get(item.id);
    if (at === undefined) appended.push(item);
    else merged[at] = item;
  }
  return appended.length > 0 ? merged.concat(appended) : merged;
}
