<script lang="ts">
  import { onMount } from 'svelte';
  import Connect from './Connect.svelte';
  import Face from './Face.svelte';
  import Home from './Home.svelte';
  import Join from './Join.svelte';
  import Notice from './Notice.svelte';
  import SignIn from './SignIn.svelte';
  import { headlessEntry } from './test-entry';
  import { JoinFlow } from './join-flow';
  import type { JoinOptions } from './media';
  import { api, ApiError, type Principal, type CallRoom } from './api';
  import { takePending } from './pending';
  import { problem } from './ui';

  // Everything before and after a call: which screen this address, this
  // browser's sign-in and the call that just ended add up to.
  let { onjoin, callError = '', joining = false, left = '' }:
    { onjoin: (id: string, name: string, options: JoinOptions, title: string) => Promise<void>; callError?: string; joining?: boolean;
      /** How the call this browser was just in ended. */ left?: '' | 'left' | 'ended' | 'removed' } = $props();
  const query = new URLSearchParams(location.search);
  const testEntry = headlessEntry(query, location.hostname, import.meta.env.DEV);
  const fragment = new URLSearchParams(location.hash.slice(1));
  // Keep pending tokens in this history entry, so a reload or sign-in does not
  // lose the invitation. They never enter a request URL or referrer.
  const kept = location.hash ? Object.fromEntries(fragment) : (history.state?.cubie ?? history.state?.quipcall ?? {});
  let magic = $state<string>(kept.magic ?? '');
  let accountInvite = $state<string>(kept.accountInvite ?? '');
  let roomInvite = $state<string>(kept.roomInvite ?? '');
  let desktop = $state(query.get('desktop') ?? '');
  let phone = $state(query.get('device') === 'phone');
  let linkedRoom = $state(query.get('room') ?? '');
  let manage = $state(query.get('manage') ?? '');
  let me = $state<Principal | null>(null);
  let name = $state('');
  let error = $state('');
  let busy = $state(false);
  let ready = $state(false);
  let offline = $state(false);
  // svelte-ignore state_referenced_locally
  let after = $state(left);
  const joinFlow = new JoinFlow();
  let entryVersion = $state(0);
  function cancelJoin() { joinFlow.cancel(); ++entryVersion; }
  const account = $derived(me?.kind === 'user' ? me : null);
  const atHome = $derived(ready && !magic && !after && !desktop && !roomInvite && !linkedRoom && !!account?.name);

  function remember() {
    const state = { ...history.state, cubie: { magic, accountInvite, roomInvite } };
    delete state.quipcall;
    history.replaceState(state, '', location.pathname + location.search);
  }
  remember();
  /** The address says where this page is, without any token. */
  function address() {
    const url = new URL(location.href);
    url.hash = '';
    if (!magic) url.pathname = '/';
    for (const [key, value] of [['room', linkedRoom], ['desktop', desktop], ['device', desktop && phone ? 'phone' : ''], ['manage', manage]]) {
      if (value) url.searchParams.set(key, value); else url.searchParams.delete(key);
    }
    history.replaceState(history.state, '', url);
    remember();
  }
  function home() {
    cancelJoin();
    linkedRoom = ''; roomInvite = ''; desktop = ''; magic = ''; accountInvite = ''; manage = ''; after = ''; error = '';
    address();
    void refresh().catch(() => {});
  }
  function choose(id: string) { cancelJoin(); linkedRoom = id; roomInvite = ''; manage = ''; after = ''; address(); }
  function accepted(room: CallRoom) { roomInvite = ''; linkedRoom = room.id; address(); }
  async function refresh() {
    const previousId = me?.id;
    try { me = await api<Principal>('/auth/me'); offline = false; }
    catch (e) { if (e instanceof ApiError && e.status === 401) { me = null; offline = false; } else throw e; }
    if (previousId && me?.id !== previousId) cancelJoin();
  }
  async function run(action: () => Promise<void>) {
    if (busy) return;
    busy = true; error = '';
    try { await action(); } catch (e) { error = problem(e); }
    finally { busy = false; }
  }
  async function identify(wanted: string, current: () => boolean): Promise<Principal> {
    let principal = me ?? await api<Principal>('/auth/guest', { name: wanted });
    if (!current()) return principal;
    if (principal.name !== wanted) principal = await api<Principal>('/auth/me', { name: wanted }, 'PATCH');
    if (current()) { me = principal; localStorage.setItem('cubie.name', principal.name); }
    return principal;
  }
  function signedIn(principal: Principal) { me = principal; accountInvite = ''; remember(); }
  async function signOut() { await api('/auth/logout', {}); cancelJoin(); me = null; }
  /** The email's link, in whichever tab it opened: sign in, then go on to
   * wherever the tab that asked for it was going. */
  const useLink = () => run(async () => {
    me = await api<Principal>('/auth/verify', { token: magic });
    magic = ''; accountInvite = '';
    if (!roomInvite && !linkedRoom && !desktop) {
      const going = takePending();
      roomInvite = going?.roomInvite ?? ''; linkedRoom = going?.room ?? '';
      desktop = going?.desktop ?? ''; phone = going?.device === 'phone';
    }
    address();
  });
  /** Straight into a call just made on Home: there is nothing left to ask. */
  async function start(room: CallRoom) {
    choose(room.id);
    try { await onjoin(room.id, account?.name ?? '', { microphone: true, camera: false }, room.name); }
    catch { /* The room's door is showing, with what went wrong. */ }
  }
  onMount(() => {
    void refresh().catch(() => { offline = true; }).finally(() => { ready = true; });
    const focus = () => { if (!busy && !joining) void refresh().catch(() => {}); };
    const hashChange = () => {
      if (!location.hash) return;
      cancelJoin();
      const next = new URLSearchParams(location.hash.slice(1));
      magic = next.get('magic') ?? magic; accountInvite = next.get('accountInvite') ?? accountInvite;
      roomInvite = next.get('roomInvite') ?? roomInvite; remember();
    };
    window.addEventListener('focus', focus); window.addEventListener('hashchange', hashChange);
    return () => { joinFlow.cancel(); window.removeEventListener('focus', focus); window.removeEventListener('hashchange', hashChange); };
  });
</script>

<div class="cw">
  {#if !atHome}<header class="brand"><img src="/favicon.svg" width="26" height="26" alt="" />Cubie</header>{/if}
  {#if !ready}
    <section class="col notice" aria-live="polite"><div class="mark spin"></div><h1>One moment…</h1></section>
  {:else if offline && !me}
    <Notice icon="offline" title="Can’t reach Cubie"><p class="sub">Check your internet connection and try again.</p>
      <button class="btn" onclick={() => refresh().catch(() => {})}>Try again</button></Notice>
  {:else if magic}
    <section class="col">
      <img src="/favicon.svg" width="88" height="88" alt="" />
      {#if error}
        <h1>This link no longer works</h1>
        <p class="sub">It may have been used already, or be more than 15 minutes old. Ask for a new code.</p>
        <button class="btn" onclick={() => { magic = ''; error = ''; address(); }}>{account ? 'Continue' : 'Sign in again'}</button>
      {:else}
        <h1>Almost there</h1>
        <p class="sub">One tap to finish signing in.</p>
        <button class="btn" disabled={busy} onclick={useLink}>{busy ? 'Signing in…' : 'Continue to Cubie'}</button>
      {/if}
    </section>
  {:else if after}
    <Notice icon="wave" title={after === 'ended' ? 'The call has ended' : after === 'removed' ? 'You were removed from the call' : 'You left the call'}>
      {#if after === 'left'}<p class="sub">You can come back while it lasts.</p><button class="btn" onclick={() => (after = '')}>Join again</button>{/if}
      {#if account}<button class="btn quiet" onclick={home}>Your rooms</button>{/if}
    </Notice>
  {:else if desktop}
    <Connect desktopId={desktop} {phone} {me} onme={signedIn} onhome={home} />
  {:else if roomInvite || linkedRoom}
    {#key entryVersion}
      <Join {testEntry} flow={joinFlow} roomId={linkedRoom} invitation={roomInvite} {me} {callError} {joining} {identify} {accepted} {onjoin}
        onme={signedIn} onhome={home} onsignout={signOut} />
    {/key}
  {:else if account?.name}
    <Home me={account} opened={manage} onme={signedIn} onroom={choose} onstart={start} onsignout={signOut} />
  {:else if account}
    <form class="col" onsubmit={(e) => { e.preventDefault(); void run(async () => { me = await api<Principal>('/auth/me', { name: name.trim() }, 'PATCH'); localStorage.setItem('cubie.name', me.name); }); }}>
      <Face {name} size={104} />
      <h1>What should people call you?</h1>
      <p class="sub">Shown on your face in calls.</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="in" bind:value={name} maxlength="64" autocomplete="name" placeholder="Your name" aria-label="Your name" disabled={busy} autofocus required />
      <button class="btn" disabled={busy || !name.trim()}>Continue</button>
      {#if error}<p class="err" role="alert">{error}</p>{/if}
    </form>
  {:else}
    <SignIn title={accountInvite ? 'You’re invited to Cubie' : 'Sign in to Cubie'} sub="Enter your email and we’ll send you a code."
      foot={accountInvite ? 'No password needed.' : 'No password needed. Cubie is invite-only for now.'} invite={accountInvite} onsignedin={signedIn}>
      {#snippet hero()}<img src="/favicon.svg" width="88" height="88" alt="" />{/snippet}
    </SignIn>
  {/if}
</div>
