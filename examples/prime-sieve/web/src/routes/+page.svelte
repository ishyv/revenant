<script>
	import wasm from '$lib/wasm';

	let limit = $state(100000);
	let checkTarget = $state(7919);

	/** @type {Uint32Array | null} */
	let primes = $state(null);
	let elapsed = $state(null);
	let isPrimeResult = $state(null);

	function runSieve() {
		const t0 = performance.now();
		primes = wasm.primes_up_to(limit);
		elapsed = (performance.now() - t0).toFixed(2);
		isPrimeResult = null;
	}

	function checkPrime() {
		isPrimeResult = wasm.is_prime(checkTarget);
	}

	let preview = $derived(
		primes
			? {
					first: Array.from(primes.slice(0, 8)),
					last: Array.from(primes.slice(-8))
				}
			: null
	);
</script>

<svelte:head>
	<title>Prime Sieve — Revenant Example</title>
</svelte:head>

<main>
	<header>
		<h1>Prime Sieve</h1>
		<p class="subtitle">
			Sieve of Eratosthenes running in WebAssembly, written in Rust, rendered by Svelte 5.
		</p>
	</header>

	<section class="card">
		<h2>Sieve</h2>
		<p class="description">Find all primes up to a limit. Try cranking it up.</p>

		<div class="row">
			<label>
				Limit
				<input type="number" bind:value={limit} min="2" max="10000000" step="1000" />
			</label>
			<button onclick={runSieve}>Run sieve</button>
		</div>

		{#if primes}
			<div class="result">
				<div class="stat">
					<span class="stat-value">{primes.length.toLocaleString()}</span>
					<span class="stat-label">primes found</span>
				</div>
				<div class="stat">
					<span class="stat-value">{elapsed}ms</span>
					<span class="stat-label">wall time</span>
				</div>
			</div>

			{#if preview}
				<div class="preview">
					<span class="preview-label">First 8</span>
					<code>{preview.first.join(', ')}</code>
				</div>
				<div class="preview">
					<span class="preview-label">Last 8</span>
					<code>{preview.last.join(', ')}</code>
				</div>
			{/if}
		{/if}
	</section>

	<section class="card">
		<h2>Primality check</h2>
		<p class="description">Call into Rust for a single deterministic check.</p>

		<div class="row">
			<label>
				Number
				<input type="number" bind:value={checkTarget} min="0" />
			</label>
			<button onclick={checkPrime}>Check</button>
		</div>

		{#if isPrimeResult !== null}
			<p class="check-result" class:prime={isPrimeResult} class:not-prime={!isPrimeResult}>
				{checkTarget} is <strong>{isPrimeResult ? 'prime' : 'not prime'}</strong>
			</p>
		{/if}
	</section>
</main>

<style>
	:global(*, *::before, *::after) {
		box-sizing: border-box;
	}

	:global(body) {
		margin: 0;
		background: #0d0d0f;
		color: #e8e8ea;
		font-family: 'Inter', system-ui, sans-serif;
		font-size: 16px;
		line-height: 1.5;
	}

	main {
		max-width: 680px;
		margin: 0 auto;
		padding: 3rem 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}

	header {
		margin-bottom: 0.5rem;
	}

	h1 {
		margin: 0 0 0.25rem;
		font-size: 2rem;
		font-weight: 700;
		letter-spacing: -0.02em;
		color: #fff;
	}

	h2 {
		margin: 0 0 0.25rem;
		font-size: 1rem;
		font-weight: 600;
		color: #fff;
	}

	.subtitle {
		margin: 0;
		color: #888;
		font-size: 0.95rem;
	}

	.card {
		background: #17171a;
		border: 1px solid #2a2a2e;
		border-radius: 10px;
		padding: 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}

	.description {
		margin: 0;
		font-size: 0.875rem;
		color: #888;
	}

	.row {
		display: flex;
		align-items: flex-end;
		gap: 0.75rem;
		flex-wrap: wrap;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
		font-size: 0.8rem;
		color: #888;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}

	input[type='number'] {
		background: #0d0d0f;
		border: 1px solid #2a2a2e;
		border-radius: 6px;
		color: #e8e8ea;
		font-size: 1rem;
		padding: 0.5rem 0.75rem;
		width: 160px;
		outline: none;
		transition: border-color 0.15s;
	}

	input[type='number']:focus {
		border-color: #6366f1;
	}

	button {
		background: #6366f1;
		border: none;
		border-radius: 6px;
		color: #fff;
		cursor: pointer;
		font-size: 0.9rem;
		font-weight: 500;
		padding: 0.55rem 1.25rem;
		transition: background 0.15s, opacity 0.15s;
	}

	button:hover:not(:disabled) {
		background: #4f46e5;
	}

	button:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.result {
		display: flex;
		gap: 2rem;
	}

	.stat {
		display: flex;
		flex-direction: column;
	}

	.stat-value {
		font-size: 1.75rem;
		font-weight: 700;
		color: #6366f1;
		line-height: 1;
	}

	.stat-label {
		font-size: 0.78rem;
		color: #666;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		margin-top: 0.2rem;
	}

	.preview {
		display: flex;
		align-items: baseline;
		gap: 0.75rem;
		font-size: 0.875rem;
	}

	.preview-label {
		color: #555;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		min-width: 52px;
	}

	code {
		color: #a5b4fc;
		font-family: 'JetBrains Mono', 'Fira Code', monospace;
		font-size: 0.85rem;
	}

	.check-result {
		margin: 0;
		font-size: 1rem;
	}

	.check-result.prime strong {
		color: #34d399;
	}

	.check-result.not-prime strong {
		color: #f87171;
	}
</style>
