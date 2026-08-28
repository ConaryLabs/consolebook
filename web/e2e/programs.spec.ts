// Browser proof of the Milestone 2 authoring slice: create a program,
// author a draft version in the structured editor, publish it, see it
// frozen, branch a new draft from it, and compare the two versions.
// All fixture data is invented.

import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';

const execFileAsync = promisify(execFile);

const BINARY = join(import.meta.dirname, '../../target/debug/consolebook-server');
const BASE = 'http://127.0.0.1:7782';
const PASSWORD = 'invented-passphrase-1';

let server: ChildProcess;
let dataDir: string;

test.beforeAll(async () => {
	dataDir = join(mkdtempSync(join(tmpdir(), 'consolebook-e2e-')), 'data');
	server = spawn(BINARY, ['--data-dir', dataDir, 'serve', '--bind', '127.0.0.1:7782'], {
		stdio: 'ignore'
	});
	for (let i = 0; i < 50; i += 1) {
		try {
			const response = await fetch(`${BASE}/api/health`);
			if (response.ok) return;
		} catch {
			// not up yet
		}
		await new Promise((resolve) => setTimeout(resolve, 200));
	}
	throw new Error('server did not become healthy');
});

test.afterAll(() => {
	server?.kill('SIGTERM');
});

async function setupCode(): Promise<string> {
	const { stdout } = await execFileAsync(BINARY, ['--data-dir', dataDir, 'setup-code']);
	return stdout.trim();
}

test('author, publish, and compare a program version', async ({ page }) => {
	// Initialize and sign in.
	await page.goto(`${BASE}/`);
	await expect(page).toHaveURL(/\/setup$/);
	await page.getByLabel('Setup code').fill(await setupCode());
	await page.getByLabel('Agency name').fill('Example County Communications');
	await page.getByLabel('Administrator username').fill('avery.admin');
	await page.getByLabel('Administrator display name').fill('Avery Admin');
	await page.getByLabel('Administrator password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Initialize installation' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('avery.admin');
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'Installation status' })).toBeVisible();

	// Programs is in the primary navigation.
	await page.getByRole('link', { name: 'Programs' }).click();
	await expect(page.getByRole('heading', { name: 'Training programs' })).toBeVisible();
	await expect(page.getByText('No programs yet.')).toBeVisible();

	// Create a program and start a blank draft.
	await page.getByLabel('Program name').fill('Example County CTO Program');
	await page.getByRole('button', { name: 'Create program' }).click();
	await expect(
		page.getByRole('heading', { name: 'Example County CTO Program' })
	).toBeVisible();
	await page.getByRole('button', { name: 'New blank draft' }).click();
	await expect(page.getByText('Draft', { exact: true })).toBeVisible();

	// Author the version in the structured editor.
	await page.getByLabel('Version label').fill('2026 rev A');
	await page.getByRole('button', { name: 'Add phase' }).click();
	await page.getByLabel('Phase name').fill('Phase One');
	await page.getByLabel('Phase description').fill('Observation.');

	await page.getByRole('button', { name: 'Add competency', exact: true }).click();
	await page.getByLabel('Competency name').fill('Emergency Call Interrogation');
	await page.getByLabel('Category').fill('Call Processing');
	await page.getByRole('button', { name: 'Add task' }).click();
	await page.getByLabel('Task prompt').fill('Processes an invented alarm call.');

	await page.getByRole('button', { name: 'Add rating scale' }).click();
	await page.getByLabel('Scale name').fill('Seven Point');
	await page.getByLabel('Anchor label').fill('Unacceptable');

	await page.getByRole('button', { name: 'Add evaluation form' }).click();
	await page.getByLabel('Form name').fill('Daily Observation Report');
	await page.getByRole('button', { name: 'Add rated competency' }).click();

	await page.getByRole('button', { name: 'Save draft' }).click();
	await expect(page.getByRole('status')).toContainText('Draft saved.');

	// Publish (through the confirmation) and see the version frozen.
	page.once('dialog', (dialog) => void dialog.accept());
	await page.getByRole('button', { name: 'Publish' }).click();
	await expect(page.getByText(/^Published /).first()).toBeVisible();
	await expect(page.getByLabel('Version label')).toBeDisabled();
	await expect(page.getByRole('button', { name: 'Save draft' })).toHaveCount(0);

	// Create a trainee from the status page and enroll them here.
	await page.getByRole('link', { name: 'Home' }).click();
	await page.getByLabel('New username').fill('jordan.trainee');
	await page.getByLabel('Display name').fill('Jordan Trainee');
	await page.getByRole('button', { name: 'Create user' }).click();
	await expect(page.getByText('First sign-in code:')).toBeVisible();

	await page.getByRole('link', { name: 'Programs' }).click();
	await page.getByRole('link', { name: 'Example County CTO Program' }).click();
	await page.getByRole('link', { name: 'v1' }).click();
	await expect(page.getByRole('heading', { name: 'Enrollments' })).toBeVisible();
	await expect(page.getByText('Nobody is enrolled in this version yet.')).toBeVisible();
	await page
		.getByLabel('Trainee to enroll')
		.selectOption({ label: 'Jordan Trainee (jordan.trainee)' });
	await page.getByRole('button', { name: 'Enroll' }).click();
	await expect(page.getByRole('cell', { name: 'Jordan Trainee' })).toBeVisible();

	// The versions table shows it published, with an export download.
	await page.getByRole('link', { name: 'Back to versions' }).click();
	await expect(page.getByText(/^Published /)).toBeVisible();
	await expect(page.getByRole('link', { name: 'Export' })).toBeVisible();

	// Branch a second draft from the published version and relabel it.
	await page.getByRole('button', { name: 'New draft from this' }).click();
	await expect(page.getByText('Draft', { exact: true })).toBeVisible();
	await page.getByLabel('Version label').fill('2026 rev B');
	await page.getByRole('button', { name: 'Save draft' }).click();
	await expect(page.getByRole('status')).toContainText('Draft saved.');
	await page.getByRole('link', { name: 'Back to versions' }).click();

	// Compare the two versions; the label change is legible.
	await page.getByLabel('Compare from').selectOption({ index: 1 });
	await page.getByLabel('Compare to').selectOption({ index: 2 });
	await page.getByRole('button', { name: 'Compare' }).click();
	await expect(page.getByRole('heading', { name: 'Compare versions' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Identity' })).toBeVisible();
	await expect(page.getByText('label changed')).toBeVisible();
});
