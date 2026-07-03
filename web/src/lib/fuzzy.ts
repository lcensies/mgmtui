// Fuzzy matching, ported from mgmt-tui's fuzzy_score: an exact substring scores 1000 - position;
// otherwise an in-order subsequence match scores low-positive; no match returns null. Higher = better.

export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase().trim();
  const t = text.toLowerCase();
  if (q === "") return 1;
  const idx = t.indexOf(q);
  if (idx >= 0) return 1000 - idx;
  let ti = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return null;
    ti = found + 1;
  }
  return 1;
}

/** Filter + rank `items` by `query` against `key(item)`, best first. Empty query keeps input order. */
export function fuzzyFilter<T>(query: string, items: T[], key: (t: T) => string): T[] {
  if (!query.trim()) return items;
  return items
    .map((item) => ({ item, score: fuzzyScore(query, key(item)) }))
    .filter((x): x is { item: T; score: number } => x.score !== null)
    .sort((a, b) => b.score - a.score)
    .map((x) => x.item);
}
