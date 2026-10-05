<script lang="ts">
  /** Observe native task status. Mounting this UI does not transfer task ownership. */
  import { Button, Text } from '@hyvnt/hyvui';
  import { errorOf } from '../contracts.js';
  import type { RuntimeError, TaskSnapshot } from '../contracts.js';
  import type { Readable } from '../store.js';
  import ErrorNotice from './ErrorNotice.svelte';
  let { task, label = 'Operation' }: {
    /** Native task store owned by its application scope. */
    task: Readable<TaskSnapshot<unknown>> & { cancel(): Promise<void>; readonly terminal: boolean };
    /** Accessible progress description. */
    label?: string;
  } = $props();
  let actionError = $state<RuntimeError>();
  let ratio = $derived($task.progress.total && BigInt($task.progress.total) > 0n
    ? Number(BigInt($task.progress.completed) * 10000n / BigInt($task.progress.total)) / 10000 : undefined);
  let terminal = $derived(!!$task && task.terminal);
  async function cancel(): Promise<void> {
    actionError = undefined;
    try { await task.cancel(); } catch (error) { actionError = errorOf(error); }
  }
</script>

<section aria-label={label} aria-live="polite">
  <Text as="h3">{label}: {$task.state}</Text>
  <p>{$task.progress.message ?? `${$task.progress.completed} / ${$task.progress.total ?? '?'}`}</p>
  <progress aria-label={`${label} progress`} value={ratio} max="1"></progress>
  {#if !terminal}
    <Button size="sm" disabled={$task.cancelRequested} onclick={cancel}>
      {$task.cancelRequested ? 'Cancellation requested' : 'Cancel operation'}
    </Button>
  {/if}
  <ErrorNotice error={actionError ?? $task.error} />
  {#if $task.summary.completed !== '0'}
    <p>{$task.summary.completed} results retained · {$task.summary.succeeded} succeeded · {$task.summary.failed} failed.</p>
  {/if}
</section>

<style>
  section { padding: 1rem; border: 1px solid var(--line); border-radius: .25rem; }
  p { color: var(--text-soft); font-size: .875rem; }
  progress { display: block; width: 100%; margin: .75rem 0; accent-color: var(--accent); }
</style>
