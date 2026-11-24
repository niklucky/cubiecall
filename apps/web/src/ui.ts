import { ApiError } from './api';

/** The avatar hue for `seed`, 0..359 (FNV-1a, as the apps do), so a person
 * keeps their colour from the Mac to the phone to here. */
export function hue(seed: string): number {
  let h = 2166136261;
  for (const b of new TextEncoder().encode(seed.toLowerCase())) h = Math.imul(h ^ b, 16777619) >>> 0;
  return h % 360;
}
/** Up to two initials: "Anna Petrova" → "AP", "max" → "M". */
export const initials = (name: string) =>
  name.trim().split(/\s+/).filter(Boolean).slice(0, 2).map(w => [...w][0].toLocaleUpperCase()).join('');
/** "Anna Petrova" → "Anna". */
export const firstName = (name: string) => name.trim().split(/\s+/)[0] ?? '';
export const validEmail = (value: string) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value.trim());
/** What to say when a request didn't get an answer the server wrote. */
export function problem(e: unknown): string {
  return e instanceof ApiError ? e.message : 'Can’t reach Cubie. Check your connection and try again.';
}
export async function copy(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}
