// 为什么单独测：Cmd+P 的排序决定「想要的文件是不是第一个」。
// 错了表现为输入 `app` 却先看到深层目录里的 snap.ts。
/** 子序列模糊匹配。返回分数（越大越好），不匹配返回 null。 */
export function fuzzyScore(query: string, target: string): number | null {
  if (!query) return 0;
  const q = query.toLowerCase();
  const t = target.toLowerCase();
  const base = t.lastIndexOf("/") + 1;
  let ti = 0;
  let score = 0;
  let prev = -2;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    score += 1;
    if (found === prev + 1) score += 3; // 连续
    if (found === 0 || "/._-".includes(t[found - 1])) score += 4; // 词首
    if (found >= base) score += 2; // 文件名部分
    prev = found;
    ti = found + 1;
  }
  const name = t.slice(base);
  if (name.startsWith(q)) score += 12;
  else if (name.includes(q)) score += 6;
  return score - target.length * 0.02;
}

export function fuzzyFilter<T>(items: T[], query: string, key: (x: T) => string, limit = 50): T[] {
  if (!query) return items.slice(0, limit);
  const scored: [number, T][] = [];
  for (const it of items) {
    const s = fuzzyScore(query, key(it));
    if (s !== null) scored.push([s, it]);
  }
  scored.sort((a, b) => b[0] - a[0]);
  return scored.slice(0, limit).map((x) => x[1]);
}
