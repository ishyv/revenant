<script lang="ts">
  import { onDestroy } from 'svelte';
  import { useApp } from '$lib/revenant';
  import type { PrimesUpTo } from '$lib/revenant';
  import type { Task } from '@revenant/client';
  import { TaskStatus } from '@revenant/client/ui';

  const app = useApp().createScope();
  const preferences = app.settings.define({ limit: 100_000 }, { key: 'prime-sieve.preferences' });
  const persistence = preferences.status;
  let limit = $state(100_000);
  let candidate = $state(97);
  let task = $state<Task<PrimesUpTo.Output.ContractValue>>();
  let result = $state<PrimesUpTo.Output.ContractValue>();
  let primality = $state<string>();
  let error = $state('');
  let elapsed = $state(0);
  let active = $state(false);
  let alive = true;
  void preferences.ready.then(() => { if (alive) limit = preferences.value.limit; }).catch(() => {});

  async function runSieve() {
    error = ''; result = undefined; active = true;
    const started = performance.now();
    try {
      preferences.set({ limit });
      await task?.dispose();
      const next = app.operations.primes.upTo.run({ limit });
      task = next;
      const value = await next.result;
      if (alive && task === next) { result = value; elapsed = performance.now() - started; }
    } catch (failure) { if (alive) error = failure instanceof Error ? failure.message : String(failure); }
    finally { if (alive) active = false; }
  }
  async function checkPrime() {
    error = '';
    try {
      const value = candidate;
      const answer = await app.operations.primes.isPrime.call({ value });
      if (alive) primality = `${value.toLocaleString()} is ${answer ? 'prime' : 'not prime'}.`;
    } catch (failure) { if (alive) error = failure instanceof Error ? failure.message : String(failure); }
  }
  onDestroy(() => { alive = false; app.dispose(); });
</script>

<svelte:head><title>Prime sieve — Revenant desktop</title></svelte:head>
<main>
  <p class="eyebrow">REVENANT / NATIVE COMPUTATION</p>
  <h1>Prime sieve</h1>
  <p>Rust does the work. Your window stays responsive, with native task progress and cancellation.</p>
  <section aria-label="Sieve controls">
    <label>Inclusive limit <input type="number" bind:value={limit} min="0" max="1000000" step="1000" /></label>
    <button disabled={active} onclick={runSieve}>Find primes</button>
    {#if task}<TaskStatus {task} label="Prime sieve" />{/if}
    {#if result}
      <h2>{result.count.toLocaleString()} primes</h2>
      <p>{elapsed.toFixed(1)} ms / through {result.limit.toLocaleString()}</p>
      <p>First eight: <code>{result.primes.slice(0, 8).join(', ')}</code></p>
      <p>Last eight: <code>{result.primes.slice(-8).join(', ')}</code></p>
    {/if}
  </section>
  <section aria-label="Primality check">
    <label>Candidate <input type="number" bind:value={candidate} min="0" max="4294967295" step="1" /></label>
    <button onclick={checkPrime}>Check primality</button>
    {#if primality}<p aria-live="polite">{primality}</p>{/if}
  </section>
  <p class="muted">Preferences: {$persistence.state}</p>
  {#if $persistence.error}<p role="alert">{$persistence.error.message}</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
</main>
<style>
  :global(body){margin:0;background:#14181e;color:#edf1f6;font-family:system-ui,sans-serif}
  main{max-width:850px;margin:auto;padding:48px 28px}h1{font-size:56px;font-weight:500;margin:20px 0}
  p{line-height:1.7}.eyebrow,.muted{font-size:12px;color:#9db5c4}.eyebrow{letter-spacing:.16em}
  section{border-top:1px solid #35424e;padding:28px 0;margin-top:28px}label{display:inline-flex;flex-direction:column;gap:8px;margin-right:16px}
  input,button{font:inherit;border:1px solid #546876;background:#202a33;color:inherit;padding:12px;border-radius:4px}button{cursor:pointer}
  button:disabled{opacity:.5;cursor:default}code{color:#a6d6ec}:global(:focus-visible){outline:2px solid #a6d6ec;outline-offset:3px}
</style>
