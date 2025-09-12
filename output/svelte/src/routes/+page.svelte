<script lang="ts">
  import { onMount } from "svelte";

  let count = $state(0);

    function onclick() {
        count++;
        }

    function attach(node: HTMLElement) {
        count;
      node.appendChild(document.createTextNode("!"));
    }

</script>

<div class="grid place-content-center min-h-screen">
  <p class="text-center" {@attach attach}>{count}</p>
  <div class="flex items-center space-x-2">
    <button class="px-4 py-2 bg-blue-500 rounded" onclick={() => count--}
      >-</button
    >
    <button class="px-4 py-2 bg-blue-500 rounded" {onclick}>+</button>
    <button class="px-4 py-2 bg-red-500 rounded" onclick={() => (count = 0)}
      >Reset</button
    >
  </div>

  {#if count > 10}
    <p class="text-center text-green-500">You reached 10!</p>
  {/if}

  {#if count < 0}
    <p class="text-center text-red-500">Negative count!</p>
  {/if}
</div>
