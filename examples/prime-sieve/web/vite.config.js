import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';
import { fileURLToPath } from 'url';
import path from 'path';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig({
	plugins: [
		wasm(),
		topLevelAwait(),
		sveltekit()
	],
	resolve: {
		alias: {
			'prime_sieve_wasm': path.join(__dirname, '../pkg/prime_sieve_wasm.js')
		}
	},
	optimizeDeps: {
		exclude: ['prime_sieve_wasm']
	}
});
