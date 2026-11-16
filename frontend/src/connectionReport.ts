const pending = new Map<string, Promise<void>>();

export function queueConnectionReport(id: string, send: () => Promise<void>): Promise<void> {
  const previous = pending.get(id) ?? Promise.resolve();
  const next = previous.catch(() => undefined).then(send);
  pending.set(id, next);
  void next.finally(() => {
    if (pending.get(id) === next) pending.delete(id);
  }).catch(() => undefined);
  return next;
}
