<script lang="ts">
    // Styles
	import "../tufte.css";
	import '../app.css';

  // Props
	let { children } = $props();

	// Assets
	import favicon from '$lib/assets/favicon.svg';

</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

{@render children?.()}
