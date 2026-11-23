<script lang="ts">
  import { api, type Principal } from './api';
  import { problem } from './ui';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import SignIn from './SignIn.svelte';

  // For apps from before they could sign in by themselves: they open this page
  // and show a code to type here. An account approves an app; a guest can't.
  let { desktopId, phone, me, onme, onhome }:
    { desktopId: string; phone: boolean; me: Principal | null; onme: (principal: Principal) => void; onhome: () => void } = $props();
  const device = $derived(phone ? 'iPhone' : 'Mac');
  let code = $state('');
  let name = $state('');
  let error = $state('');
  let busy = $state(false);
  let done = $state(false);
  const account = $derived(me?.kind === 'user' ? me : null);
  const ready = $derived(/^[a-f0-9]{8}$/i.test(code.trim()) && !!(account?.name || name.trim()));
  async function connect() {
    if (busy || !ready || !account) return;
    busy = true; error = '';
    try {
      if (!account.name) onme(await api<Principal>('/auth/me', { name: name.trim() }, 'PATCH'));
      await api('/auth/desktop/approve', { id: desktopId, code: code.trim() });
      done = true;
    } catch (e) { error = problem(e); }
    finally { busy = false; }
  }
</script>

{#if done}
  <Notice icon="check" good title="You’re connected"><p class="sub">Go back to Cubie on your {device}.</p>
    {#if phone}<button class="btn" onclick={() => { location.href = 'quipcall://signed-in'; }}>Open the app</button>{:else}<button class="btn quiet" onclick={onhome}>Your rooms</button>{/if}</Notice>
{:else if !account}
  <SignIn title="Sign in to connect your {device}" sub="Enter your email and we’ll send you a code." pending={{ desktop: desktopId, device: phone ? 'phone' : '' }} onsignedin={onme}>
    {#snippet hero()}<div class="mark"><Icon name={phone ? 'phone' : 'laptop'} size={34} /></div>{/snippet}
  </SignIn>
{:else}
  <form class="col" onsubmit={(e) => { e.preventDefault(); void connect(); }}>
    <div class="mark"><Icon name={phone ? 'phone' : 'laptop'} size={34} /></div>
    <h1>Connect Cubie on your {device}</h1>
    <p class="sub">Type the code Cubie shows on your {device}.</p>
    {#if !account.name}<input class="in" bind:value={name} maxlength="64" autocomplete="name" placeholder="Your name" aria-label="Your name" disabled={busy} required />{/if}
    <!-- svelte-ignore a11y_autofocus -->
    <input class="in" bind:value={code} autocomplete="off" autocapitalize="characters" spellcheck="false" maxlength="8" placeholder="8-character code" aria-label="Code shown in the app" disabled={busy} autofocus required />
    <button class="btn" disabled={busy || !ready}>{busy ? 'Connecting…' : `Connect as ${account.name || name.trim() || 'you'}`}</button>
    {#if error}<p class="err" role="alert">{error}</p>{/if}
    <p class="foot">Signed in as {account.email}. Don’t use a code someone sent you.</p>
  </form>
{/if}
