import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
	server: {
		// Development convenience only; production serves everything from
		// the Rust executable.
		proxy: {
			'/api': 'http://127.0.0.1:7770'
		}
	}
});
