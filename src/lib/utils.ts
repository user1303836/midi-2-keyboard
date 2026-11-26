import { clsx, type ClassValue } from "clsx";

/** Joins class names (conditionals, arrays, objects). */
export function cn(...inputs: ClassValue[]): string {
  return clsx(inputs);
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Short random id, unique enough for config objects. */
export function newId(prefix: string): string {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return `${prefix}-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

export function hex(byte: number): string {
  return byte.toString(16).toUpperCase().padStart(2, "0");
}

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** Formats milliseconds compactly: 250 ms, 1.5 s, 2 min. */
export function formatMs(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${+(ms / 1000).toFixed(2)} s`;
  return `${+(ms / 60_000).toFixed(1)} min`;
}

/** Structural equality for plain JSON-like data. */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, index) => deepEqual(item, b[index]));
  }
  const keysA = Object.keys(a as object);
  const keysB = Object.keys(b as object);
  if (keysA.length !== keysB.length) return false;
  return keysA.every((key) =>
    deepEqual((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]),
  );
}

/** Moves an array item, returning a new array. */
export function move<T>(items: readonly T[], from: number, to: number): T[] {
  const next = items.slice();
  const [item] = next.splice(from, 1);
  next.splice(clamp(to, 0, next.length), 0, item);
  return next;
}

export function download(filename: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
