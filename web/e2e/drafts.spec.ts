// Browser proof of Milestone 3 slice 3: starting the daily draft from a
// session, autosaved collaborative content with visible attribution,
// ownership transfer, and the submission that freezes the draft. All
// fixture data is invented.

import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';

const execFileAsync = promisify(execFile);

const BINARY = join(import.meta.dirname, '../../target/debug/consolebook-server');
const BASE = 'http://127.0.0.1:7785';
const PASSWORD = 'invented-passphrase-1';
const JORDAN_PASSWORD = 'trainer-passphrase-3';
const ROWAN_PASSWORD = 'trainer-passphrase-4';

let server: ChildProcess;
let dataDir: string;

test.beforeAll(async () => {
	dataDir = join(mkdtempSync(join(tmpdir(), 'consolebook-e2e-')), 'data');
	server = spawn(BINARY, ['--data-dir', dataDir, 'serve', '--bind', '127.0.0.1:7785'], {
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

const content = {
	name: 'Example County CTO Program',
	label: '2026 rev A',
	description: 'Invented program for draft e2e.',
	phases: [
		{ name: 'Phase One', description: 'Observation.', presentation_number: 1 },
		{ name: 'Phase Two', description: 'Guided performance.', presentation_number: 2 }
	],
	phase_transitions: [{ from_phase: 'Phase One', to_phase: 'Phase Two', kind: 'advance' }],
	competencies: [
		{
			category: 'Call processing',
			name: 'Emergency Call Interrogation',
			description: 'Obtains and verifies location, callback, and nature.',
			tasks: [{ prompt: 'Processes an invented structure-fire call.', citations: [] }],
			citations: []
		},
		{
			category: 'Radio',
			name: 'Radio Discipline',
			description: 'Uses clear text and unit identifiers.',
			tasks: [{ prompt: 'Dispatches an invented medical call.', citations: [] }],
			citations: []
		}
	],
	rating_scales: [
		{
			name: 'Standard 1-7',
			kind: 'anchored_numeric',
			min_value: 1,
			max_value: 7,
			anchors: [
				{ value: 1, label: 'Unacceptable', definition: 'Contrary to training.' },
				{ value: 4, label: 'Meets standards', definition: 'To the invented standard.' },
				{ value: 7, label: 'Superior', definition: 'Beyond the invented standard.' }
			]
		},
		{
			name: 'Check',
			kind: 'pass_fail',
			min_value: null,
			max_value: null,
			anchors: [
				{ value: 0, label: 'Fail', definition: 'Did not perform.' },
				{ value: 1, label: 'Pass', definition: 'Performed.' }
			]
		}
	],
	rating_modifiers: [
		{
			code: 'NRT',
			label: 'Not responding to training',
			description: 'Remedial attention documented in the narrative.'
		}
	],
	evaluation_forms: [
		{
			record_type: 'daily_report',
			name: 'Daily Observation Report',
			instructions: 'Rate observed performance.',
			competencies: [
				{ competency: 'Emergency Call Interrogation', rating_scale: 'Standard 1-7' },
				{ competency: 'Radio Discipline', rating_scale: 'Check' }
			],
			narratives: [
				{ prompt: 'Most acceptable performance.', required: true },
				{ prompt: 'Least acceptable performance.', required: false }
			]
		}
	],
	citations: []
};

async function resetAndLogin(page: Page, username: string, resetCode: string, password: string) {
	await page.goto(`${BASE}/reset`);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Reset code').fill(resetCode);
	await page.getByLabel('New password').fill(password);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill(username);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in' }).click();
}

test('draft, collaborate, transfer, and submit a daily evaluation', async ({ page }) => {
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

	// Seed the published program, people, enrollment, assignment, and an
	// open session over the API; the cookie rides page.request.
	const program = await (
		await page.request.post(`${BASE}/api/programs`, { data: { name: content.name } })
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
	const jordan = await (
		await page.request.post(`${BASE}/api/users`, {
			data: { username: 'jordan.trainer', display_name: 'Jordan Trainer', role: 'trainer' }
		})
	).json();
	const rowan = await (
		await page.request.post(`${BASE}/api/users`, {
			data: { username: 'rowan.trainer', display_name: 'Rowan Trainer', role: 'trainer' }
		})
	).json();
	const enrollment = await (
		await page.request.post(`${BASE}/api/program-versions/${version.id}/enrollments`, {
			data: { user_id: trainee.id }
		})
	).json();
	await page.request.post(`${BASE}/api/enrollments/${enrollment.id}/assignments`, {
		data: { trainer_user_id: jordan.id }
	});
	await page.request.post(`${BASE}/api/enrollments/${enrollment.id}/sessions`, {
		data: {
			business_date: '2026-06-02',
			timezone: 'America/Chicago',
			local_start: '2026-06-02T07:00',
			trainer_user_ids: [jordan.id, rowan.id]
		}
	});
	await page.getByRole('button', { name: 'Sign out' }).click();

	// Jordan starts the draft from their own session list.
	await resetAndLogin(page, 'jordan.trainer', jordan.reset_code, JORDAN_PASSWORD);
	await expect(page.getByRole('heading', { name: 'My sessions' })).toBeVisible();
	await page.getByRole('button', { name: 'Start draft' }).click();
	await expect(page).toHaveURL(new RegExp('/drafts/\\d+$'));
	await expect(
		page.getByRole('heading', { name: 'Daily Observation Report' })
	).toBeVisible();
	await expect(page.getByText('created the draft')).toBeVisible();

	// Rate, mark a modifier, write the narrative; autosave reports Saved
	// and the contribution appears in the attribution stream.
	await page
		.getByLabel('Rate Emergency Call Interrogation')
		.selectOption({ label: '4 — Meets standards' });
	await page.getByLabel('Rate Radio Discipline').selectOption({ label: 'Pass' });
	await page
		.getByLabel('Most acceptable performance.')
		.fill('Ran the invented structure fire cleanly.');
	await expect(page.getByText('Saved', { exact: true })).toBeVisible();
	await page.reload();
	await expect(page.getByLabel('Rate Emergency Call Interrogation')).toHaveValue('4');
	await expect(page.getByLabel('Most acceptable performance.')).toHaveValue(
		'Ran the invented structure fire cleanly.'
	);
	await expect(page.getByText('Jordan Trainer contributed')).toBeVisible();

	// Hand the draft to Rowan; the submit control leaves with ownership.
	await page.getByLabel('Transfer ownership').selectOption({ label: 'Rowan Trainer' });
	await page.getByRole('button', { name: 'Transfer', exact: true }).click();
	await expect(page.getByText('transferred ownership to')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Submit for review' })).toHaveCount(0);
	const draftUrl = page.url();

	// Rowan finds the session on their list, opens the draft, and submits.
	await page.getByRole('link', { name: 'Home' }).click();
	await page.getByRole('button', { name: 'Sign out' }).click();
	await resetAndLogin(page, 'rowan.trainer', rowan.reset_code, ROWAN_PASSWORD);
	await expect(page.getByRole('heading', { name: 'My sessions' })).toBeVisible();
	await page.getByRole('link', { name: 'Open draft' }).click();
	await expect(page).toHaveURL(draftUrl);
	await page
		.getByLabel('Least acceptable performance.')
		.fill('Slow unit identifier once; corrected.');
	await expect(page.getByText('Saved', { exact: true })).toBeVisible();
	await page.getByRole('button', { name: 'Submit for review' }).click();
	await expect(page.getByText('Submitted for review', { exact: true })).toBeVisible();
	await expect(page.getByLabel('Most acceptable performance.')).toBeDisabled();
	await expect(page.getByLabel('Rate Emergency Call Interrogation')).toBeDisabled();
	await expect(page.getByRole('button', { name: 'Submit for review' })).toHaveCount(0);

	// The freeze survives a reload; the covered session still shows.
	await page.reload();
	await expect(page.getByText('Submitted for review', { exact: true })).toBeVisible();
	await expect(page.getByText('Session 2026-06-02:')).toBeVisible();
});
