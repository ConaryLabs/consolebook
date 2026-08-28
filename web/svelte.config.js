import adapter from '@sveltejs/adapter-static';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	kit: {
		// Single-page app: every route is client-rendered and unknown paths
		// fall back to index.html, served by the Rust executable.
		adapter: adapter({
			fallback: 'index.html'
		})
	}
};

export default config;
