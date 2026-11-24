<script lang="ts">
  // Six boxes drawn over one real field, so typing, pasting and the phone's
  // "From Mail" suggestion all work as in any text field.
  let { value = $bindable(''), disabled = false, bad = false, oncomplete }:
    { value?: string; disabled?: boolean; bad?: boolean; oncomplete: (code: string) => void } = $props();
  let focused = $state(false);
  function typed(e: Event & { currentTarget: HTMLInputElement }) {
    const digits = e.currentTarget.value.replace(/\D/g, '').slice(0, 6);
    e.currentTarget.value = digits;
    value = digits;
    if (digits.length === 6) oncomplete(digits);
  }
  function ready(input: HTMLInputElement) { input.focus(); }
</script>
<div class="code" class:bad>
  {#each [0, 1, 2, 3, 4, 5] as i (i)}<b class:cur={focused && i === Math.min(value.length, 5)}>{value[i] ?? ''}</b>{/each}
  <input {@attach ready} {value} {disabled} oninput={typed} onfocus={() => (focused = true)} onblur={() => (focused = false)}
    inputmode="numeric" autocomplete="one-time-code" maxlength="12" aria-label="6-digit code" />
</div>
