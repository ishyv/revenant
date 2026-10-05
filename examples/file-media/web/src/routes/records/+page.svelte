<script lang="ts">
  import { onDestroy } from 'svelte';
  import { useApp } from '$lib/revenant';
  import type { RecordsNormalize } from '$lib/revenant';
  import type { Task } from '@revenant/client';
  import { TaskStatus } from '@revenant/client/ui';

  const app = useApp().createScope();
  const records = app.collection<RecordsNormalize.Input.ContractValue>([
    { id: 'docs', title: '  write useful documentation  ', done: false },
    { id: 'ship', title: '  ship the damn thing ', done: false },
    { id: 'sleep', title: ' sleep ', done: true }
  ]);
  const selection = records.selection();
  let task = $state<Task<RecordsNormalize.Output.ContractValue>>();
  let error = $state('');
  let running = $state(false);
  let alive = true;

  async function normalize() {
    error = ''; running = true;
    const selected = [...selection.items];
    try {
      // This is a small application collection; native folder batches use
      // FileCollection selection tokens instead of copying a dataset to JS.
      for (const record of selected) {
        await task?.dispose();
        task = app.operations.records.normalize.run(record);
        const result = await task.result;
        if (!alive) return;
        records.set(records.items.map((item) => item.id === result.id ? result : item));
      }
    } catch (failure) { if (alive) error = failure instanceof Error ? failure.message : String(failure); }
    finally { if (alive) running = false; }
  }
  onDestroy(() => { alive = false; app.dispose(); });
</script>

<svelte:head><title>Ordinary records — Revenant desktop</title></svelte:head>
<main>
  <a href="/">← Back to files</a>
  <h1>Ordinary records.<br />One Rust operation.</h1>
  <p>Revenant owns the native runtime. Your library exports an application and ordinary functions.</p>
  <section aria-label="Record collection">
    {#each $records.items as record (record.id)}
      <label><input type="checkbox" disabled={running} checked={$selection.includes(record)} onchange={(event) => selection.set(event.currentTarget.checked ? [...selection.items, record] : selection.items.filter((item) => item !== record))} /><span><strong>{record.title}</strong><small>{record.id} / {record.done ? 'done' : 'open'}</small></span></label>
    {/each}
  </section>
  <div class="actions"><button disabled={!$selection.length || running} onclick={normalize}>Normalize selected titles</button><button disabled={running} onclick={() => selection.set(records.items)}>Select all</button></div>
  {#if task}<TaskStatus {task} label="Normalize record" />{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <p class="note">Hover the generated operation in your editor for its Rustdoc, payload types and Rust source link. Identity and completion are preserved.</p>
</main>
<style>
  main{max-width:850px;margin:auto;padding:50px 28px}:global(body){margin:0;background:#14181e;color:#edf1f6;font-family:system-ui,sans-serif}
  a{color:#a6d6ec;font-size:13px}h1{font-size:clamp(36px,6vw,60px);line-height:1.1;font-weight:500}p{color:#a6b8c7;line-height:1.7}
  section{border-top:1px solid #35424e;margin:30px 0}label{display:flex;gap:18px;padding:20px 0;border-bottom:1px solid #35424e}label strong{font-weight:400;font-size:18px}label small{display:block;color:#9db5c4;font-size:12px;margin-top:8px}
  .actions{display:flex;gap:16px;flex-wrap:wrap}button{font:inherit;border:1px solid #546876;background:#202a33;color:inherit;padding:12px;border-radius:4px;cursor:pointer}button:disabled{opacity:.5;cursor:default}.note{font-size:13px;margin-top:35px}:global(:focus-visible){outline:2px solid #a6d6ec;outline-offset:3px}
</style>
