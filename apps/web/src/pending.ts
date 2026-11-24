/** Where someone was going when they asked for a sign-in email. The email's
 * link opens a new tab, which has only this to go by. Kept in this browser
 * for half an hour, and gone once used. */
export type Pending = { room?: string; roomInvite?: string; desktop?: string; device?: string };
const KEY = 'cubie.pending';
const LASTS = 30 * 60_000;
type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;
const browser = (): Store | undefined => { try { return localStorage; } catch { return undefined; } };

export function savePending(pending: Pending, store = browser(), now = Date.now()) {
  try {
    if (Object.values(pending).some(Boolean)) store?.setItem(KEY, JSON.stringify({ ...pending, at: now }));
    else store?.removeItem(KEY);
  } catch { /* Private browsing: the code still works, in the tab that asked. */ }
}
export function takePending(store = browser(), now = Date.now()): Pending | null {
  try {
    const raw = store?.getItem(KEY);
    store?.removeItem(KEY);
    if (!raw) return null;
    const { at, ...pending } = JSON.parse(raw) as Pending & { at: number };
    return now - at < LASTS ? pending : null;
  } catch { return null; }
}
export function clearPending(store = browser()) { try { store?.removeItem(KEY); } catch { /* Nothing kept. */ } }
