import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';
import { revenantSdk } from '@revenant/client/vite';

export default defineConfig({
  plugins: [revenantSdk(), sveltekit({
    preprocess: vitePreprocess(),
    adapter: adapter({ pages: '../build/web', assets: '../build/web', fallback: 'index.html' })
  })],
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    fs: { allow: ['..'] },
    watch: { ignored: ['**/.revenant/desktop/**', '**/.revenant/target/**'] }
  }
});
