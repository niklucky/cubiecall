<script lang="ts">
  import Icon from './Icon.svelte';
  import { hue, initials } from './ui';
  // A person's or a room's face: initials on their colour, as in the apps.
  let { name, size = 40 }: { name: string; size?: number } = $props();
  const h = $derived(hue(name.trim()));
</script>
{#if name.trim()}
  <span class="face" style:width="{size}px" style:height="{size}px" style:font-size="{Math.round(size * 0.38)}px"
    style:background="linear-gradient(135deg, hsl({h} 55% 52%), hsl({(h + 29) % 360} 60% 38%))">{initials(name)}</span>
{:else}
  <span class="face none" style:width="{size}px" style:height="{size}px"><Icon name="user" size={Math.round(size * 0.4)} /></span>
{/if}
