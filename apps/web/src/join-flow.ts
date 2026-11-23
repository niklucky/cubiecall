import type { CallRoom, Principal, Entry } from './api';
import type { JoinOptions } from './media';

export type JoinRequest = {
  roomId: string;
  invitation: string;
  name: string;
  title: string;
  options: JoinOptions;
};
export type JoinEffects = {
  identify: (name: string, current: () => boolean) => Promise<Principal>;
  redeem: (token: string) => Promise<CallRoom>;
  accepted: (room: CallRoom) => void;
  stopPreview: () => void;
  join: (roomId: string, name: string, options: JoinOptions, title: string) => Promise<void>;
};

/** What the link leads to, asked without claiming anything. */
export async function loadJoinEntry(
  target: Pick<JoinRequest, 'roomId' | 'invitation'>,
  check: (body: { invite: string } | { room: string }) => Promise<Entry>,
): Promise<Entry> {
  if (!target.invitation && !target.roomId) return { state: 'unavailable' };
  return check(target.invitation ? { invite: target.invitation } : { room: target.roomId });
}

/** Navigation cancels the continuation of an action, including a response that
 * arrives after teardown. Already-sent requests may finish on the server, but
 * must never trigger another request, change the route or join a call. */
export class JoinFlow {
  private generation = 0;
  cancel() { ++this.generation; }
  async run(input: JoinRequest, effects: JoinEffects): Promise<void> {
    const generation = ++this.generation;
    const current = () => generation === this.generation;
    // Capture the entry and media choices before the first asynchronous step.
    const request = { ...input, options: { ...input.options } };
    try {
      const principal = await effects.identify(request.name, current);
      if (!current()) return;
      if (request.invitation) {
        const room = await effects.redeem(request.invitation);
        if (!current()) return;
        request.roomId = room.id;
        request.title = room.name;
        effects.accepted(room);
        if (!current()) return;
      }
      effects.stopPreview();
      if (!current()) return;
      await effects.join(request.roomId, principal.name, request.options, request.title);
    } catch (error) {
      if (current()) throw error;
    }
  }
}
