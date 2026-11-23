<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type Entry, type Principal, type CallRoom } from './api';
  import type { TestEntry } from './test-entry';
  import { loadJoinEntry, type JoinFlow } from './join-flow';
  import { MediaPreview, type PreviewState, type JoinOptions } from './media';
  import { copy, firstName, problem } from './ui';
  import Face from './Face.svelte';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import SignIn from './SignIn.svelte';

  // The door to a call. A link asks for a name and nothing else; a private
  // room asks who you are first. It never offers an app and never opens one.
  let { roomId, invitation, me, callError, joining, flow, testEntry, identify, accepted, onjoin, onme, onhome, onsignout }:
    { roomId: string; invitation: string; me: Principal | null; callError: string; joining: boolean; flow: JoinFlow; testEntry?: TestEntry;
      identify: (name: string, current: () => boolean) => Promise<Principal>; accepted: (room: CallRoom) => void;
      onjoin: (id: string, name: string, options: JoinOptions, title: string) => Promise<void>;
      onme: (principal: Principal) => void; onhome: () => void; onsignout: () => Promise<void> } = $props();
  let entry = $state<Entry | null>(null);
  let loading = $state(true);
  let error = $state('');
  let busy = $state(false);
  let wantsAccount = $state(false);
  let copied = $state(false);
  let name = $state(localStorage.getItem('cubie.name') ?? '');
  let identityId: string | undefined;
  // The microphone is asked for when the call starts; the camera shows here first.
  let microphone = $state(true);
  let media = $state.raw<PreviewState>({ audioBusy: false, videoBusy: false, audioError: '', videoError: '' });
  let devices = $state<MediaDeviceInfo[]>([]);
  let audioId = $state('');
  let videoId = $state('');
  let choosing = $state(false);
  let video = $state<HTMLVideoElement>();
  const preview = new MediaPreview(navigator.mediaDevices, state => { media = state; });
  const capable = typeof RTCPeerConnection !== 'undefined' && !!navigator.mediaDevices;
  let request = 0;
  let alive = true;
  let attempt = 0;
  let autojoined = false;
  const working = $derived(busy || joining);
  const account = $derived(me?.kind === 'user' ? me : null);
  const room = $derived(entry?.room);
  const host = $derived(firstName(room?.host ?? ''));
  const microphones = $derived(devices.filter(d => d.kind === 'audioinput' && d.deviceId));
  const cameras = $derived(devices.filter(d => d.kind === 'videoinput' && d.deviceId));
  type View = 'loading' | 'offline' | 'unsupported' | 'signin' | 'account' | 'door' | 'uninvited' | 'another' | 'used' | 'expired' | 'removed' | 'full' | 'unavailable';
  const view = $derived.by((): View => {
    if (!capable) return 'unsupported';
    if (!entry) return loading ? 'loading' : 'offline';
    switch (entry.state) {
      // A room invitation made for one email: that account, or none yet.
      case 'signin': return account ? 'another' : 'signin';
      case 'private': return account ? 'uninvited' : 'signin';
      case 'invite': return room?.kind === 'private' && !account ? 'signin' : wantsAccount ? 'account' : 'door';
      case 'ready': return wantsAccount && !account ? 'account' : 'door';
      default: return entry.state;
    }
  });
  const title = $derived(
    !room ? 'Join the call'
    : account && !account.name ? 'You’re in. What’s your name?'
    : room.mine ? (room.kind === 'direct' ? 'Your call' : room.name)
    : room.kind === 'direct' ? (host ? `${host} invited you to a call` : 'You’re invited to a call')
    : room.name);
  const sub = $derived(
    !room ? ''
    : account && !account.name ? (host && !room.mine ? `${host} and the others will see it in ${room.kind === 'direct' ? 'the call' : room.name}.` : 'The others will see it in the call.')
    : room.mine ? (room.kind === 'direct' ? 'Send the link to someone, then join.' : '')
    : room.kind === 'direct' || !host ? '' : `${host}’s room`);

  async function loadEntry() {
    const current = ++request;
    loading = true;
    try {
      const result = await loadJoinEntry({ invitation, roomId }, body => api<Entry>('/entry', body));
      if (alive && current === request) { entry = result; error = ''; }
    } catch {
      if (alive && current === request) entry = null;
    } finally { if (alive && current === request) loading = false; }
  }
  $effect(() => { void roomId; void invitation; void me?.id; void loadEntry(); });
  $effect(() => {
    if (me && identityId !== me.id) { identityId = me.id; if (me.name || me.kind === 'user') name = me.name; }
  });
  $effect(() => { if (video) video.srcObject = media.video ? new MediaStream([media.video]) : null; });
  // Only a call is worth a camera: any other screen lets go of it.
  $effect(() => { if (view !== 'door') preview.stop(); });
  $effect(() => {
    if (testEntry && !autojoined && view === 'door' && !invitation) {
      autojoined = true; name = testEntry.name; void proceed(testEntry.options);
    }
  });
  async function enumerate() {
    try { const list = await navigator.mediaDevices?.enumerateDevices(); if (alive) devices = list ?? []; } catch { /* The switches still work. */ }
  }
  async function camera() {
    await preview.set('video', !media.video && !media.videoBusy, videoId);
    await enumerate();
  }
  onMount(() => {
    void enumerate();
    const stop = () => { flow.cancel(); ++attempt; busy = false; preview.stop(); };
    window.addEventListener('pagehide', stop);
    navigator.mediaDevices?.addEventListener('devicechange', enumerate);
    return () => {
      alive = false; ++request; ++attempt; flow.cancel(); preview.dispose();
      window.removeEventListener('pagehide', stop);
      navigator.mediaDevices?.removeEventListener('devicechange', enumerate);
    };
  });
  async function proceed(testOptions?: JoinOptions) {
    if (working || view !== 'door' || !name.trim()) return;
    const operation = ++attempt;
    const current = () => alive && operation === attempt;
    busy = true; error = '';
    const options: JoinOptions = testOptions ?? { microphone, camera: !!media.video,
      audioDeviceId: audioId || undefined,
      videoDeviceId: media.video?.getSettings().deviceId || videoId || undefined };
    try {
      await flow.run({ roomId, invitation, name: name.trim(), title: room?.name ?? roomId, options }, {
        identify, accepted,
        redeem: token => api<CallRoom>('/invites/redeem', { token }),
        stopPreview: () => preview.stop(),
        join: onjoin,
      });
    } catch (e) {
      if (current()) { error = problem(e); await loadEntry(); }
    } finally { if (current()) busy = false; }
  }
  function signedIn(principal: Principal) { wantsAccount = false; onme(principal); }
  async function another() { await onsignout(); wantsAccount = true; }
</script>

{#snippet again(label = 'Check again')}
  <button class="btn quiet" disabled={loading} onclick={() => void loadEntry()}>{label}</button>
{/snippet}

{#if view === 'loading'}
  <section class="col notice" aria-live="polite"><div class="mark spin"></div><h1>One moment…</h1></section>
{:else if view === 'offline'}
  <Notice icon="offline" title="Can’t reach Cubie"><p class="sub">Check your internet connection and try again.</p><button class="btn" disabled={loading} onclick={() => void loadEntry()}>Try again</button></Notice>
{:else if view === 'unsupported'}
  <Notice icon="globe" title="Open this link in Safari or Chrome"><p class="sub">This app’s built-in browser can’t make calls.</p>
    <button class="btn" onclick={async () => { copied = await copy(location.href); }}>{copied ? 'Copied' : 'Copy link'}</button></Notice>
{:else if view === 'signin'}
  {#if room}
    <SignIn title={host ? `${host} invited you to ${room.name}` : `You’re invited to ${room.name}`}
      sub={entry?.inviteKind === 'member' ? `This call is private. Enter the email ${host || 'the owner'} invited, and we’ll send you a code to get in.` : 'This call is private. Enter your email and we’ll send you a code to get in.'}
      invite={invitation} pending={invitation ? { roomInvite: invitation } : { room: roomId }} onsignedin={signedIn}>
      {#snippet hero()}<Face name={room.name} size={88} />{/snippet}
    </SignIn>
  {:else}
    <SignIn title="This call is private" sub="Sign in to join it. Enter your email and we’ll send you a code."
      pending={{ room: roomId }} onsignedin={signedIn}>
      {#snippet hero()}<div class="mark"><Icon name="lock" size={34} /></div>{/snippet}
    </SignIn>
  {/if}
{:else if view === 'account'}
  <SignIn title="Sign in to Cubie" sub="Enter your email and we’ll send you a code." pending={invitation ? { roomInvite: invitation } : { room: roomId }} onsignedin={signedIn}>
    {#snippet hero()}<img src="/favicon.svg" width="88" height="88" alt="" />{/snippet}
  </SignIn>
  <button class="lk back" onclick={() => (wantsAccount = false)}>Back to the call</button>
{:else if view === 'uninvited'}
  <Notice icon="lock" title="You need an invitation"><p class="sub">This call is private. Ask the person who sent the link to invite <b>{account?.email}</b>.</p>
    {@render again()}<p class="foot"><button class="lk" onclick={another}>Use another account</button></p></Notice>
{:else if view === 'another'}
  <Notice icon="user" title="This invitation is for another email"><p class="sub">You’re signed in as <b>{account?.email}</b>. Sign in with the email {host || 'the owner'} invited.</p>
    <button class="btn" onclick={another}>Use another email</button></Notice>
{:else if view === 'used'}
  <Notice icon="check" title="This invitation was already used"><p class="sub">Each one works for one person. If that was you, sign in. If not, ask for a new one.</p>
    {#if !account}<button class="btn" onclick={() => (wantsAccount = true)}>Sign in</button>{:else}{@render again()}{/if}</Notice>
{:else if view === 'expired'}
  <Notice icon="clock" title="This invitation has expired"><p class="sub">Ask the person who invited you for a new link.</p></Notice>
{:else if view === 'removed'}
  <Notice icon="lock" title="You no longer have access"><p class="sub">The room’s owner removed you. Ask them if you need to come back.</p></Notice>
{:else if view === 'full'}
  <Notice icon="users" title="This call is for two"><p class="sub">Someone already joined it. If that was you, use the same browser, or sign in.</p>
    {#if !account}<button class="btn" onclick={() => (wantsAccount = true)}>Sign in</button>{/if}{@render again()}</Notice>
{:else if view === 'unavailable'}
  <Notice icon="clock" title="This call has ended"><p class="sub">Or the link is incomplete. Ask the person who invited you for a new one.</p>
    {#if account}<button class="btn quiet" onclick={onhome}>Your rooms</button>{/if}</Notice>
{:else}
  <form class="col door" onsubmit={(e) => { e.preventDefault(); void proceed(); }}>
    {#if media.video}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video class="face mirror" bind:this={video} autoplay muted playsinline aria-label="Your camera"></video>
    {:else}
      <Face {name} size={104} />
    {/if}
    <div class="toggles">
      <button type="button" class="tg" class:on={microphone} aria-pressed={microphone} disabled={working} onclick={() => (microphone = !microphone)}>
        <span><Icon name={microphone ? 'mic' : 'mic-off'} /></span>{microphone ? 'Mic on' : 'Mic off'}</button>
      <button type="button" class="tg" class:on={!!media.video} aria-pressed={!!media.video} disabled={working} onclick={camera}>
        <span><Icon name={media.video ? 'video' : 'video-off'} /></span>{media.videoBusy ? 'Starting…' : media.video ? 'Camera on' : 'Camera off'}</button>
    </div>
    {#if media.videoError}<p class="err" role="alert">{media.videoError}</p>{/if}
    <h1>{title}</h1>
    {#if sub}<p class="sub">{sub}</p>{/if}
    <input class="in" name="name" bind:value={name} maxlength="64" autocomplete="name" placeholder="Your name" aria-label="Your name" required disabled={working} />
    <button class="btn" disabled={working || loading || !name.trim() || media.videoBusy}>{working ? 'Joining…' : 'Join call'}</button>
    {#if error || callError}<p class="err" role="alert">{error || callError}</p>{/if}
    {#if media.video}<p class="foot">Only you see your camera until you join.</p>{/if}
    {#if microphones.length > 1 || cameras.length > 1}
      {#if choosing}
        <div class="devices">
          {#if microphones.length > 1}<label>Microphone<select bind:value={audioId} disabled={working}><option value="">Automatic</option>{#each microphones as d, i (d.deviceId)}<option value={d.deviceId}>{d.label || `Microphone ${i + 1}`}</option>{/each}</select></label>{/if}
          {#if cameras.length > 1}<label>Camera<select bind:value={videoId} disabled={working || media.videoBusy} onchange={() => { if (media.video) void preview.set('video', true, videoId); }}><option value="">Automatic</option>{#each cameras as d, i (d.deviceId)}<option value={d.deviceId}>{d.label || `Camera ${i + 1}`}</option>{/each}</select></label>{/if}
        </div>
      {:else}
        <p class="foot"><button type="button" class="lk" onclick={() => (choosing = true)}>Change microphone or camera</button></p>
      {/if}
    {/if}
    <p class="foot">
      {#if account}Signed in as {account.email}{:else}Have a Cubie account? <button type="button" class="lk" disabled={working} onclick={() => (wantsAccount = true)}>Sign in</button>{/if}
    </p>
  </form>
{/if}
