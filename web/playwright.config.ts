import { defineConfig } from '@playwright/test';

// End-to-end proof against the real compiled server binary. Run
// `npm run build` and `cargo build` first; the spec starts the server
// itself on a scratch data directory.
export default defineConfig({
	testDir: './e2e',
	timeout: 60_000,
	use: {
		// Prefer an environment-provided Chromium (CONSOLEBOOK_E2E_CHROMIUM)
		// over downloading a browser per Playwright version.
		launchOptions: process.env.CONSOLEBOOK_E2E_CHROMIUM
			? { executablePath: process.env.CONSOLEBOOK_E2E_CHROMIUM }
			: {}
	}
});
