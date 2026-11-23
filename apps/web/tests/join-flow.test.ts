import test from 'node:test';
import assert from 'node:assert/strict';
import { loadJoinEntry, JoinFlow, type JoinEffects, type JoinRequest } from '../src/join-flow.ts';
import { savePending, takePending } from '../src/pending.ts';
import type { Principal, CallRoom } from '../src/api.ts';
const principal: Principal = { id: 'guest', kind: 'guest', name: 'Guest', email: null, admin: false };
const room: CallRoom = { id: 'claimed-room', name: 'Project', kind: 'private', owner: 'owner', guest: null, closed: false, expires: null };
function deferred<T>() { let resolve!: (v: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { resolve, promise }; }
function setup() {
  const calls: string[] = [];
  const input: JoinRequest = { roomId: '', invitation: 'invite', name: 'Guest', title: 'Project', options: { microphone: false, camera: true, videoDeviceId: 'camera-one' } };
  const effects: JoinEffects = {
    identify: async () => principal,
    redeem: async token => { calls.push(`redeem:${token}`); return room; },
    accepted: value => { calls.push(`accepted:${value.id}`); },
    stopPreview: () => { calls.push('stop'); },
    join: async (id, name, options) => { calls.push(`join:${id}:${name}:${options.videoDeviceId}`); },
  };
  return { input, effects, calls, flow: new JoinFlow() };
}
test('leaving during identification prevents every continuation effect', async () => {
  const { input, effects, calls, flow } = setup();
  const pending = deferred<Principal>(); effects.identify = () => pending.promise;
  const run = flow.run(input, effects);
  flow.cancel(); // The lobby cancels on navigation, and Join cancels on teardown.
  pending.resolve(principal); await run;
  assert.deepEqual(calls, []);
});
test('an in-flight redemption cannot redirect or join after cancellation', async () => {
  const { input, effects, calls, flow } = setup();
  const started = deferred<void>(), pending = deferred<CallRoom>();
  effects.redeem = () => { started.resolve(); return pending.promise; };
  const run = flow.run(input, effects); await started.promise;
  flow.cancel(); pending.resolve(room); await run;
  assert.deepEqual(calls, []);
});
test('a current attempt completes with its original entry and device choices', async () => {
  const { input, effects, calls, flow } = setup();
  const pending = deferred<Principal>(); effects.identify = () => pending.promise;
  const run = flow.run(input, effects);
  input.invitation = 'a-different-invitation'; input.options.videoDeviceId = 'other-camera';
  pending.resolve(principal); await run;
  assert.deepEqual(calls, ['redeem:invite', 'accepted:claimed-room', 'stop', 'join:claimed-room:Guest:camera-one']);
});
test('a failed join reaches the page that asked for it', async () => {
  const { input, effects, flow } = setup();
  effects.join = async () => { throw new Error('This call is full or unavailable'); };
  await assert.rejects(flow.run(input, effects), /full or unavailable/);
});
test('an invitation is checked before joining, and a room link by its room', async () => {
  const { input } = setup();
  const seen: unknown[] = [];
  const entry = await loadJoinEntry(input, async body => { seen.push(body); return { state: 'private' }; });
  assert.equal(entry.state, 'private');
  assert.equal((await loadJoinEntry({ roomId: 'room', invitation: '' }, async body => { seen.push(body); return { state: 'ready' }; })).state, 'ready');
  assert.deepEqual(seen, [{ invite: 'invite' }, { room: 'room' }]);
  assert.equal((await loadJoinEntry({ roomId: '', invitation: '' }, async () => { throw new Error('not asked'); })).state, 'unavailable');
});
test('the email link finds where its reader was going, once, for half an hour', () => {
  const kept = new Map<string, string>();
  const store = { getItem: (k: string) => kept.get(k) ?? null, setItem: (k: string, v: string) => { kept.set(k, v); }, removeItem: (k: string) => { kept.delete(k); } };
  savePending({ roomInvite: 'invite' }, store, 1_000);
  assert.deepEqual(takePending(store, 60_000), { roomInvite: 'invite' });
  assert.equal(takePending(store, 60_000), null);
  savePending({ room: 'room' }, store, 1_000);
  assert.equal(takePending(store, 1_000 + 31 * 60_000), null);
  // Going nowhere in particular forgets an earlier destination.
  savePending({ room: 'room' }, store, 1_000); savePending({}, store, 2_000);
  assert.equal(takePending(store, 3_000), null);
});
