export function orderInputs<T extends { channel_id: string }>(inputs: T[], order: string[]): T[] {
  const positions = new Map(order.map((id, index) => [id, index]));
  return [...inputs].sort((a, b) =>
    (positions.get(a.channel_id) ?? Number.MAX_SAFE_INTEGER) -
    (positions.get(b.channel_id) ?? Number.MAX_SAFE_INTEGER),
  );
}

export function moveInput(order: string[], id: string, direction: -1 | 1): string[] {
  const from = order.indexOf(id);
  const to = from + direction;
  if (from < 0 || to < 0 || to >= order.length) return order;
  const next = [...order];
  [next[from], next[to]] = [next[to], next[from]];
  return next;
}
