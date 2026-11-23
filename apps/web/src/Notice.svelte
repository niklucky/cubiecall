<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from './Icon.svelte';
  // One thing to say, and at most one thing to do about it.
  let { icon, title, good = false, children }: { icon: IconName; title: string; good?: boolean; children?: Snippet } = $props();
</script>
<section class="col notice" aria-live="polite">
  <div class="mark" class:good><Icon name={icon} size={34} /></div>
  <h1>{title}</h1>
  {@render children?.()}
</section>
