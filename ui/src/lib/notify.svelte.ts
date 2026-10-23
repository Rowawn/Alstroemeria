export type Toast = { id: number; message: string; kind: "info" | "error" };

export const toasts = $state<Toast[]>([]);
let next = 1;

export function toast(message: string, kind: "info" | "error" = "info") {
  const id = next++;
  toasts.push({ id, message, kind });
  setTimeout(() => dismiss(id), kind === "error" ? 9000 : 4000);
}

export function dismiss(id: number) {
  const i = toasts.findIndex((t) => t.id === id);
  if (i >= 0) toasts.splice(i, 1);
}
