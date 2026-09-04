import { access } from 'node:fs/promises';
import { createServer } from 'node:net';
import { expect, test } from '@playwright/test';
import { BINARY, startServer } from './server';

test('missing executable and early exit fail at startup', async () => {
	await expect(startServer({ binary: `${BINARY}.missing` })).rejects.toThrow(/ENOENT/);
	// Node rejects the application's CLI arguments and exits without listening.
	await expect(startServer({ binary: process.execPath })).rejects.toThrow(/Consolebook exited/);
});

test('an occupied listener cannot be mistaken for the spawned server', async () => {
	const listener = createServer((socket) => {
		socket.end('HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok');
	});
	await new Promise<void>((resolve) => listener.listen(0, '127.0.0.1', resolve));
	try {
		const address = listener.address();
		if (!address || typeof address === 'string') throw new Error('expected TCP listener');
		const failure = await startServer({ bind: `127.0.0.1:${address.port}` }).then(
			async (server) => {
				await server.stop();
				throw new Error('unexpectedly started on an occupied port');
			},
			(error: Error) => error
		);
		expect(failure.message).toContain('Consolebook exited');
		expect(failure.message).toContain('binding');
		expect(failure.message).not.toContain('setup_code');
		expect(failure.message).not.toMatch(/[0-9a-f]{32}/);
	} finally {
		await new Promise<void>((resolve, reject) => {
			listener.close((error) => error ? reject(error) : resolve());
		});
	}
});

test('parallel installations have distinct listeners and awaited cleanup', async () => {
	const first = await startServer();
	try {
		const second = await startServer();
		try {
			expect(first.url).not.toBe(second.url);
		} finally {
			await second.stop();
		}
		await expect(access(second.root)).rejects.toThrow();
		expect(() => process.kill(second.pid, 0)).toThrow();
	} finally {
		await first.stop();
	}
	await expect(access(first.root)).rejects.toThrow();
	expect(() => process.kill(first.pid, 0)).toThrow();
});
