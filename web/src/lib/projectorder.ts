// Sidebar project ordering. Kept pure so the drag interaction stays a thin wrapper: the list is
// arranged by an explicit name order, and anything not in that order keeps its incoming
// (alphabetical) position behind the arranged ones.

/** Sort `list` by `order`; names absent from `order` follow, in their original relative order. */
export function orderProjects<T extends { name: string }>(list: T[], order: string[]): T[] {
  const rank = new Map(order.map((name, i) => [name, i]));
  return list
    .map((item, i) => ({ item, i }))
    .sort((a, b) => {
      const ra = rank.get(a.item.name);
      const rb = rank.get(b.item.name);
      if (ra !== undefined && rb !== undefined) return ra - rb;
      if (ra !== undefined) return -1; // arranged projects come first
      if (rb !== undefined) return 1;
      return a.i - b.i; // both unarranged: keep the incoming order
    })
    .map((x) => x.item);
}

/** Move `name` to sit where `target` currently is, returning the full new order. */
export function moveProject(names: string[], name: string, target: string): string[] {
  const from = names.indexOf(name);
  const to = names.indexOf(target);
  if (from < 0 || to < 0 || from === to) return names;
  const next = names.slice();
  next.splice(from, 1);
  next.splice(to, 0, name);
  return next;
}
