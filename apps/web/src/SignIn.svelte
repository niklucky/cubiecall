<script lang="ts">
  import type { Snippet } from 'svelte';
  import { onMount } from 'svelte';
  import { api, type Principal } from './api';
  import { savePending, clearPending, type Pending } from './pending';
  import { problem, validEmail } from './ui';
  import Icon from './Icon.svelte';
  import CodeInput from './CodeInput.svelte';

  // Email, then the six digits from it. Signing in and signing up are the
  // same two screens: an invitation decides which it is, not the person.
  // An address with no account and no invitation is told so on the first.
  let { title, sub, foot = 'No password needed.', invite = '', pending = {}, hero, onsignedin }:
    { title: string; sub: string; foot?: string; invite?: string; pending?: Pending; hero?: Snippet;
      onsignedin: (principal: Principal) => void } = $props();
  let step = $state<'email' | 'code'>('email');
  let email = $state('');
  let challenge = '';
  let code = $state('');
  let error = $state('');
  let busy = $state(false);
  let wait = $state(0);
  let alive = true;
  const valid = $derived(validEmail(email));

  async function send() {
    if (busy || !valid) return;
    busy = true; error = '';
    try {
      const sent = await api<{ id: string }>('/auth/request', { email: email.trim(), invite: invite || undefined });
      if (!alive) return;
      challenge = sent.id; code = ''; step = 'code'; wait = 30;
      // The email's link opens another tab: tell it where this one was going.
      savePending(pending);
    } catch (e) { if (alive) error = problem(e); }
    finally { busy = false; }
  }
  async function check(value: string) {
    if (busy) return;
    busy = true; error = '';
    try {
      const principal = await api<Principal>('/auth/verify', { id: challenge, code: value });
      if (!alive) return;
      clearPending();
      onsignedin(principal);
    } catch (e) { if (alive) { error = problem(e); code = ''; } }
    finally { busy = false; }
  }
  // Signed in through the email's link in another tab: carry on here too.
  async function elsewhere() {
    if (step !== 'code' || busy || document.visibilityState !== 'visible') return;
    try {
      const principal = await api<Principal>('/auth/me');
      if (alive && step === 'code' && principal.kind === 'user') onsignedin(principal);
    } catch { /* Not signed in yet. */ }
  }
  $effect(() => {
    if (wait <= 0) return;
    const timer = setTimeout(() => { wait -= 1; }, 1000);
    return () => clearTimeout(timer);
  });
  onMount(() => {
    window.addEventListener('focus', elsewhere);
    document.addEventListener('visibilitychange', elsewhere);
    return () => { alive = false; window.removeEventListener('focus', elsewhere); document.removeEventListener('visibilitychange', elsewhere); };
  });
</script>

{#if step === 'email'}
  <form class="col" onsubmit={(e) => { e.preventDefault(); void send(); }}>
    {@render hero?.()}
    <h1>{title}</h1>
    <p class="sub">{sub}</p>
    <!-- svelte-ignore a11y_autofocus -->
    <input class="in" type="email" name="email" bind:value={email} placeholder="Your email" autocomplete="email" autocapitalize="off" spellcheck="false" aria-label="Your email" disabled={busy} autofocus required />
    <button class="btn" disabled={busy || !valid}>{busy ? 'Sending…' : 'Continue'}</button>
    {#if error}<p class="err" role="alert">{error}</p>{/if}
    <p class="foot">{foot}</p>
  </form>
{:else}
  <section class="col">
    <div class="mark"><Icon name="mail" size={34} /></div>
    <h1>Check your email</h1>
    <p class="sub">We sent a 6-digit code to <b>{email.trim()}</b>.</p>
    <CodeInput bind:value={code} disabled={busy} bad={!!error} oncomplete={check} />
    {#if error}<p class="err" role="alert">{error}</p>{/if}
    <p class="foot">
      <button class="lk" type="button" disabled={busy || wait > 0} onclick={send}>{wait > 0 ? `Send a new code in ${wait} s` : 'Send a new code'}</button>
      · <button class="lk" type="button" disabled={busy} onclick={() => { step = 'email'; error = ''; }}>Use a different email</button>
    </p>
    <details class="help"><summary>Didn’t get it?</summary>
      <p>Look in spam, and check the address above. If several codes came, the newest is the one that works.</p>
    </details>
  </section>
{/if}
