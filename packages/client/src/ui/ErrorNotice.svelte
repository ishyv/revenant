<script lang="ts">
  /** Render structured native failures and offer retries only when the host allows them. */
  import { Button, ErrorState } from '@hyvnt/hyvui';
  import type { RuntimeError } from '../contracts.js';
  let { error, title = 'That operation hit a wall.', onretry }: {
    /** Classified native error. */
    error?: RuntimeError;
    /** Presentation title. */
    title?: string;
    /** Optional retry action for retryable native failures. */
    onretry?: () => void;
  } = $props();
</script>

{#if error}
  <div role="alert">
    <ErrorState {title} description={error.message} />
    {#if error.retryable && onretry}<Button size="sm" onclick={onretry}>Try again</Button>{/if}
  </div>
{/if}
