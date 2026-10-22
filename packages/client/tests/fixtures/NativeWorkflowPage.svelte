<script>
  import { untrack } from 'svelte';
  import { provideApp } from './NativeWorkflowFacade.mjs';
  import Page from '../../../../examples/native-workflows/web/src/routes/+page.svelte';
  let { app } = $props();
  provideApp(untrack(() => app));
</script>
<Page />
