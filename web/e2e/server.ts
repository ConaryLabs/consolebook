// Own the child and its scratch directory together. Never return readiness
// from another process that happens to answer on the requested port.
import { execFile, spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { promisify, stripVTControlCharacters } from 'node:util';

export const BINARY = join(import.meta.dirname, '../../target/debug/consolebook-server');
export const execFileAsync = promisify(execFile);

export async function startServer({ binary = BINARY, bind = '127.0.0.1:0' } = {}) {
	const root = await mkdtemp(join(tmpdir(), 'consolebook-e2e-'));
	const dataDir = join(root, 'data');
	const child = spawn(binary, ['--data-dir', dataDir, 'serve', '--bind', bind], {
		stdio: ['ignore', 'pipe', 'ignore'],
		env: { ...process.env, RUST_LOG: 'info', NO_COLOR: '1' }
	});
	// Retain bounded startup diagnostics only; setup-code output is discarded.
	const diagnostics: string[] = [];
	let starting = true;
	let spawnError: Error | undefined;
	let stopped = false;
	const closed = new Promise<Error>((resolve) => {
		child.once('error', (error) => {
			spawnError = error;
		});
		child.once('close', (code, signal) => {
			stopped = true;
			resolve(
				spawnError ?? new Error(
					`Consolebook exited (code ${code}, signal ${signal})\n${diagnostics.join('\n')}`
				)
			);
		});
	});
	const lines = createInterface({ input: child.stdout });
	const listening = new Promise<string>((resolve) => {
		lines.on('line', (line) => {
			const plain = stripVTControlCharacters(line);
			if (starting && /\b(ERROR|WARN)\b/.test(plain) && !plain.includes('setup_code')) {
				diagnostics.push(plain.slice(0, 2_000));
				if (diagnostics.length > 10) diagnostics.shift();
			}
			// The server emits this only after successfully binding its listener.
			// Parsing here discovers an ephemeral test address, never domain data.
			const match = plain.match(/\blistening addr=(127\.0\.0\.1:\d+)$/);
			if (match) resolve(`http://${match[1]}`);
		});
	});
	let cleaned = false;
	async function stop() {
		if (cleaned) return;
		if (!stopped && child.pid) {
			child.kill('SIGTERM');
			const force = setTimeout(() => child.kill('SIGKILL'), 5_000);
			try {
				await closed;
			} finally {
				clearTimeout(force);
			}
		}
		await closed;
		lines.close();
		await rm(root, { recursive: true, force: true });
		cleaned = true;
	}
	let deadline: ReturnType<typeof setTimeout> | undefined;
	try {
		const url = await Promise.race([
			listening,
			closed.then((error) => {
				throw error;
			}),
			new Promise<never>((_, reject) => {
				deadline = setTimeout(
					() => reject(new Error('Consolebook did not announce a listener within 10s')),
					10_000
				);
			})
		]);
		starting = false;
		const response = await fetch(`${url}/api/health`, { signal: AbortSignal.timeout(5_000) });
		if (!response.ok) throw new Error(`Consolebook health returned ${response.status}`);
		if (stopped) throw await closed;
		return {
			url,
			root,
			pid: child.pid!,
			stop,
			async setupCode() {
				const { stdout } = await execFileAsync(binary, ['--data-dir', dataDir, 'setup-code']);
				return stdout.trim();
			}
		};
	} catch (error) {
		await stop();
		throw error;
	} finally {
		clearTimeout(deadline);
	}
}
