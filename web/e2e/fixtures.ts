import { test as base, expect } from '@playwright/test';
import { startServer } from './server';

export { expect };

export const test = base.extend<{
	server: Awaited<ReturnType<typeof startServer>>;
	setupCode: string;
}>({
	server: async ({}, use) => {
		const server = await startServer();
		try {
			await use(server);
		} finally {
			// Scratch records and credentials are discarded on success and failure.
			await server.stop();
		}
	},
	baseURL: async ({ server }, use) => {
		await use(server.url);
	},
	setupCode: async ({ server }, use) => {
		await use(await server.setupCode());
	}
});
