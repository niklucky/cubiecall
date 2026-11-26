<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type CallRoom, type Principal, type RoomDetail } from './api';
  import { copy, firstName, problem, validEmail } from './ui';
  import Face from './Face.svelte';
  import Icon from './Icon.svelte';

  // Signed in, no link: one button to start a call, and your rooms.
  let { me, opened = '', onme, onroom, onstart, onsignout }:
    { me: Principal; opened?: string; onme: (principal: Principal) => void;
      /** To a room's door. */ onroom: (id: string) => void;
      /** Straight into a call just made. */ onstart: (room: CallRoom) => void;
      onsignout: () => Promise<void> } = $props();
  const origin = location.origin === import.meta.env.VITE_CUBIE_LEGACY_URL
    ? import.meta.env.VITE_CUBIE_PUBLIC_URL || location.origin
    : location.origin;
  const link = (id: string) => `${origin}/?room=${id}`;
  let rooms = $state<CallRoom[]>([]);
  let detail = $state<RoomDetail | null>(null);
  let started = $state<CallRoom | null>(null);
  let menu = $state(false);
  let adding = $state(false);
  let renaming = $state(false);
  let newName = $state('');
  let newKind = $state<'private' | 'public'>('private');
  let roomName = $state('');
  let myName = $state('');
  let invitation = $state('');
  let friend = $state('');
  let friendLink = $state('');
  let copied = $state('');
  let error = $state('');
  let busy = $state(false);
  let alive = true;
  const mine = $derived(detail?.room.owner === me.id);
  const kinds = { private: 'Private', public: 'Anyone with the link', direct: 'A call for two' } as const;

  async function run(action: () => Promise<void>) {
    if (busy) return;
    busy = true; error = '';
    try { await action(); } catch (e) { if (alive) error = problem(e); }
    finally { busy = false; }
  }
  async function refresh() { const list = await api<CallRoom[]>('/rooms'); if (alive) rooms = list; }
  async function open(id: string) {
    const found = await api<RoomDetail>(`/rooms/${id}`);
    if (!alive) return;
    detail = found; roomName = found.room.name; invitation = ''; renaming = false; copied = ''; menu = false;
  }
  const again = () => open(detail!.room.id);
  async function clip(text: string, what: string) { if (await copy(text)) { copied = what; setTimeout(() => { if (copied === what) copied = ''; }, 2500); } }
  const start = () => run(async () => {
    started = await api<CallRoom>('/rooms', { name: `Call with ${firstName(me.name) || 'me'}`, kind: 'direct' });
  });
  const create = () => run(async () => {
    const room = await api<CallRoom>('/rooms', { name: newName.trim(), kind: newKind });
    newName = ''; adding = false;
    await refresh(); await open(room.id);
  });
  const invite = () => run(async () => {
    const made = await api<{ url: string }>(`/rooms/${detail!.room.id}/invites`, { kind: 'guest' });
    try { await again(); } finally { invitation = made.url; }
    await clip(made.url, 'invitation');
  });
  const rename = () => run(async () => {
    await api(`/rooms/${detail!.room.id}`, { name: roomName.trim() }, 'PATCH');
    await again(); await refresh();
  });
  const close = () => {
    if (confirm(`Close ${detail!.room.name}? Everyone in it is disconnected, and its link stops working.`)) void run(async () => {
      await api(`/rooms/${detail!.room.id}`, {}, 'DELETE');
      detail = null; await refresh();
    });
  };
  /** The sheet goes first: what it shows follows `started`. */
  function joinStarted() { const room = started; started = null; if (room) onstart(room); }
  const saveName = () => run(async () => { onme(await api<Principal>('/auth/me', { name: myName.trim() }, 'PATCH')); menu = false; });
  onMount(() => {
    void run(async () => { await refresh(); if (opened) await open(opened); });
    const focus = () => { if (!busy) void refresh().catch(() => {}); };
    window.addEventListener('focus', focus);
    return () => { alive = false; window.removeEventListener('focus', focus); };
  });
</script>

<header class="top">
  {#if detail}
    <button class="chip" onclick={() => { detail = null; error = ''; }}><Icon name="left" size={18} />Rooms</button>
  {:else}
    <span class="brand"><img src="/favicon.svg" width="26" height="26" alt="" />Cubie</span>
  {/if}
  <button class="chip" aria-expanded={menu} onclick={() => { menu = !menu; myName = me.name; }}>{me.name}<Face name={me.name} size={28} /></button>
</header>
{#if menu}
  <div class="home menu">
    <form onsubmit={(e) => { e.preventDefault(); void saveName(); }}>
      <label>Your name<input class="in" bind:value={myName} maxlength="64" autocomplete="name" required disabled={busy} /></label>
      <button class="sm pri" disabled={busy || !myName.trim() || myName.trim() === me.name}>Save</button>
    </form>
    <p class="foot">Signed in as {me.email} · <button class="lk" disabled={busy} onclick={() => run(onsignout)}>Sign out</button></p>
  </div>
{/if}

<main class="home">
  {#if error}<p class="err" role="alert">{error}</p>{/if}
  {#if detail}
    {@const room = detail.room}
    <div class="li head"><Face name={room.name} size={52} /><div><h1>{room.name}</h1><small>{room.kind === 'private' ? 'Private · only people you invite' : kinds[room.kind]}</small></div></div>
    <button class="btn" onclick={() => onroom(room.id)}>Join call</button>
    {#if room.kind === 'private'}
      {#if mine}
        <button class="btn quiet" disabled={busy} onclick={invite}>Invite someone</button>
        {#if invitation}
          <div class="linkbox">{invitation}</div>
          <p class="foot">{copied === 'invitation' ? 'Copied. ' : ''}Send it to one person. They sign up with their email, then they’re in. <button class="lk" onclick={() => clip(invitation, 'invitation')}>Copy again</button></p>
        {:else}
          <p class="foot">Each invitation link works for one person. They sign up with their email, then they’re in.</p>
        {/if}
      {/if}
    {:else}
      <button class="btn quiet" onclick={() => clip(link(room.id), 'link')}>{copied === 'link' ? 'Copied' : 'Copy link'}</button>
      <p class="foot">Anyone with the link joins with just their name{room.kind === 'direct' ? ': one person, for this call' : ''}.</p>
    {/if}
    {#if room.expires && mine}
      <p class="foot">This call’s link works until {new Date(room.expires * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })} unless it’s in use.
        <button class="lk" disabled={busy} onclick={() => run(async () => { await api(`/rooms/${room.id}/keep`, {}); await again(); await refresh(); })}>Keep it as a room</button></p>
    {/if}
    {#if mine && (detail.members?.length || detail.invites?.length)}
      <h3>People</h3>
      {#each detail.members ?? [] as person (person.id)}
        <div class="li"><Face name={person.name} size={36} /><div><b>{person.name}</b><small>{person.revoked ? 'Removed' : person.kind === 'guest' ? 'Joined without an account' : 'Has access'}</small></div>
          <button class="sm" disabled={busy} onclick={() => run(async () => { try { await api(`/rooms/${room.id}/members/${person.id}`, {}, 'DELETE'); } finally { await again(); } })}>{person.revoked ? 'Disconnect again' : 'Remove'}</button></div>
      {/each}
      {#each detail.invites ?? [] as waiting (waiting.hash)}
        <div class="li"><span class="face none" style:width="36px" style:height="36px"><Icon name="clock" size={16} /></span><div><b class="plain">{waiting.email ?? 'Invitation not used yet'}</b><small>Expires {new Date(waiting.expires * 1000).toLocaleDateString([], { day: 'numeric', month: 'long' })}</small></div>
          <button class="sm" disabled={busy} onclick={() => run(async () => { await api(`/rooms/${room.id}/invites/${waiting.hash}`, {}, 'DELETE'); await again(); })}>Cancel</button></div>
      {/each}
    {/if}
    {#if mine}
      {#if renaming}
        <form class="inline" onsubmit={(e) => { e.preventDefault(); void rename(); }}>
          <input class="in" aria-label="Room name" bind:value={roomName} maxlength="64" required disabled={busy} />
          <button class="sm pri" disabled={busy || !roomName.trim()}>Save</button>
        </form>
      {/if}
      <p class="foot">{#if !renaming}<button class="lk" onclick={() => (renaming = true)}>Rename</button> · {/if}<a class="lk" href={`quipcall://open/${room.id}`}>Open in the Mac app</a> · <button class="lk bad" disabled={busy} onclick={close}>Close this room</button></p>
    {:else}
      <p class="foot"><a class="lk" href={`quipcall://open/${room.id}`}>Open in the Mac app</a></p>
    {/if}
  {:else}
    <h1>Hi, {firstName(me.name)}</h1>
    <button class="btn" disabled={busy} onclick={start}>Start a call</button>
    <p class="foot">You get a link to send. Anyone can join it with just their name.</p>
    <h3>Your rooms<button class="lk" onclick={() => (adding = !adding)}>{adding ? 'Cancel' : '+ New room'}</button></h3>
    {#if adding}
      <form class="new" onsubmit={(e) => { e.preventDefault(); void create(); }}>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="in" aria-label="Room name" bind:value={newName} maxlength="64" placeholder="Room name" required disabled={busy} autofocus />
        <div class="seg" role="radiogroup" aria-label="Who can join">
          <button type="button" role="radio" aria-checked={newKind === 'private'} onclick={() => (newKind = 'private')}>Private</button>
          <button type="button" role="radio" aria-checked={newKind === 'public'} onclick={() => (newKind = 'public')}>Anyone with the link</button>
        </div>
        <p class="foot">{newKind === 'private' ? 'Only people you invite can join. They need an account.' : 'Anyone with the link joins with just their name.'}</p>
        <button class="btn" disabled={busy || !newName.trim()}>Create room</button>
      </form>
    {/if}
    {#each rooms as room (room.id)}
      <div class="li"><Face name={room.name} size={40} />
        <button class="rowlink" disabled={busy} onclick={() => run(() => open(room.id))}><b>{room.name}</b><small>{kinds[room.kind]}</small></button>
        <button class="sm" onclick={() => onroom(room.id)}>Join</button></div>
    {:else}
      {#if !adding}<p class="foot">No rooms yet. A room is a link that stays: make one for the people you call often.</p>{/if}
    {/each}
    {#if me.admin}
      <h3>Invite someone to Cubie</h3>
      <form class="inline" onsubmit={(e) => { e.preventDefault(); void run(async () => { friendLink = (await api<{ url: string }>('/auth/invites', { email: friend.trim() })).url; await clip(friendLink, 'friend'); }); }}>
        <input class="in" type="email" aria-label="Their email" bind:value={friend} placeholder="Their email" required disabled={busy} />
        <button class="sm pri" disabled={busy || !validEmail(friend)}>Invite</button>
      </form>
      {#if friendLink}<div class="linkbox">{friendLink}</div><p class="foot">{copied === 'friend' ? 'Copied. ' : ''}Send them this link. It works for that email, for a week.</p>{/if}
    {/if}
    <p class="foot apps"><a class="lk" href="/download/">Cubie for Mac</a> · <a class="lk" href="/download/#windows">Cubie for Windows</a></p>
  {/if}
</main>

{#if started}
  {@const room = started}
  <div class="sheet" role="dialog" aria-modal="true" aria-label="Your call is ready">
    <div>
      <div class="mark good"><Icon name="check" size={34} /></div>
      <h1>Your call is ready</h1>
      <p class="sub">Send this link to anyone. They join with just their name.</p>
      <div class="linkbox">{link(room.id)}</div>
      <button class="btn" onclick={() => clip(link(room.id), 'call')}>{copied === 'call' ? 'Copied' : 'Copy link'}</button>
      <button class="btn quiet" onclick={joinStarted}>Join call</button>
      <p class="foot"><button class="lk" onclick={() => (started = null)}>Not now</button></p>
    </div>
  </div>
{/if}
