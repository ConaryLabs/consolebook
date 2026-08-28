// Browser proof of Milestone 3 slice 1: a role-bundled trainer created
// through the form, an assignment recorded on the enrollment page, phase
// history driven through the graph-aware picker, and the assigned trainer
// reading their trainee through the scoped view. All fixture data is
// invented.

import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';

const execFileAsync = promisify(execFile);

const BINARY = join(import.meta.dirname, '../../target/debug/consolebook-server');
const BASE = 'http://127.0.0.1:7783';
const PASSWORD = 'invented-passphrase-1';
const TRAINER_PASSWORD = 'trainer-passphrase-3';

let server: ChildProcess;
let dataDir: string;

test.beforeAll(async () => {
	dataDir = join(mkdtempSync(join(tmpdir(), 'consolebook-e2e-')), 'data');
	server = spawn(BINARY, ['--data-dir', dataDir, 'serve', '--bind', '127.0.0.1:7783'], {
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

/** Two phases with an advance edge and a remediation loop. */
const content = {
	name: 'Example County CTO Program',
	label: '2026 rev A',
	description: 'Invented program for lifecycle e2e.',
	phases: [
		{ name: 'Phase One', description: 'Observation.', presentation_number: 1 },
		{ name: 'Phase Two', description: 'Guided performance.', presentation_number: 2 }
	],
	phase_transitions: [
		{ from_phase: 'Phase One', to_phase: 'Phase Two', kind: 'advance' },
		{ from_phase: 'Phase Two', to_phase: 'Phase One', kind: 'remediation' }
	],
	competencies: [
		{
			category: '',
			name: 'Emergency Call Interrogation',
			description: 'Obtains and verifies location, callback, and nature.',
			tasks: [{ prompt: 'Processes an invented structure-fire call.', citations: [] }],
			citations: []
		}
	],
	rating_scales: [
		{
			name: 'Narrative Assessment',
			kind: 'narrative_only',
			min_value: null,
			max_value: null,
			anchors: []
		}
	],
	rating_modifiers: [],
	evaluation_forms: [],
	citations: []
};

test('assign a trainer, record phase history, and read it scoped', async ({ page }) => {
	// Initialize and sign in as the administrator.
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

	// Create the trainer through the role-aware form and keep their code.
	await page.getByLabel('New username').fill('jordan.trainer');
	await page.getByLabel('Display name').fill('Jordan Trainer');
	await page.getByLabel('Role').selectOption('trainer');
	await page.getByLabel('Employee identifier').fill('T-7');
	await page.getByRole('button', { name: 'Create user' }).click();
	await expect(page.getByText('First sign-in code:')).toBeVisible();
	const trainerCode = ((await page.locator('p.code-out code').textContent()) ?? '').trim();

	// Seed the published program, the trainee, and the enrollment over the
	// API; the signed-in session's cookies ride page.request.
	const program = await (
		await page.request.post(`${BASE}/api/programs`, {
			data: { name: content.name }
		})
	).json();
	const version = await (
		await page.request.post(`${BASE}/api/programs/${program.id}/versions`, { data: content })
	).json();
	await page.request.post(`${BASE}/api/program-versions/${version.id}/publish`, { data: {} });
	const trainee = await (
		await page.request.post(`${BASE}/api/users`, {
			data: { username: 'taylor.trainee', display_name: 'Taylor Trainee' }
		})
	).json();
	await page.request.post(`${BASE}/api/program-versions/${version.id}/enrollments`, {
		data: { user_id: trainee.id }
	});

	// The enrollment page opens from the version's enrollee list.
	await page.getByRole('link', { name: 'Programs' }).click();
	await page.getByRole('link', { name: 'Example County CTO Program' }).click();
	await page.getByRole('link', { name: 'v1' }).click();
	await page.getByRole('link', { name: 'Taylor Trainee' }).click();
	await expect(page.getByRole('heading', { name: 'Taylor Trainee' })).toBeVisible();
	await expect(page.getByText('no phase entered yet')).toBeVisible();

	// Assign the trainer.
	await page
		.getByLabel('Trainer to assign')
		.selectOption({ label: 'Jordan Trainer (jordan.trainer)' });
	await page.getByRole('button', { name: 'Assign' }).click();
	await expect(page.getByRole('cell', { name: 'Jordan Trainer' })).toBeVisible();

	// Record the entry advance, then pause training.
	await page.getByLabel('Target phase').selectOption({ label: 'Phase One' });
	await page.getByRole('button', { name: 'Record phase event' }).click();
	await expect(page.getByText('entry → Phase One')).toBeVisible();
	await expect(page.getByText('current phase: Phase One')).toBeVisible();
	await page.getByLabel('Phase action').selectOption('pause');
	await page.getByRole('button', { name: 'Record phase event' }).click();
	await expect(page.getByText('Paused')).toBeVisible();

	// The trainer sets a password through the reset flow and signs in.
	await page.getByRole('link', { name: 'Home' }).click();
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.goto(`${BASE}/reset`);
	await page.getByLabel('Username').fill('jordan.trainer');
	await page.getByLabel('Reset code').fill(trainerCode);
	await page.getByLabel('New password').fill(TRAINER_PASSWORD);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('jordan.trainer');
	await page.getByLabel('Password').fill(TRAINER_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();

	// The assignment notice and the scoped trainee view are theirs.
	await expect(page.getByText('New training assignment: Taylor Trainee')).toBeVisible();
	await expect(page.getByRole('heading', { name: 'My trainees' })).toBeVisible();
	await page.getByRole('link', { name: 'Taylor Trainee' }).click();
	await expect(page.getByRole('heading', { name: 'Taylor Trainee' })).toBeVisible();
	await expect(page.getByText('entry → Phase One')).toBeVisible();
	await expect(page.getByText('Paused')).toBeVisible();

	// Reading is not recording: the trainer gets no mutation controls.
	await expect(page.getByRole('button', { name: 'Record phase event' })).toHaveCount(0);
	await expect(page.getByRole('button', { name: 'Assign' })).toHaveCount(0);
	await expect(page.getByRole('button', { name: 'Withdraw' })).toHaveCount(0);
});
