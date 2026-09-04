// Browser proof of Milestone 3 slice 2: recording a training session with
// agency-local times, the overlap invariant surfacing in the interface,
// closing with a disposition, and the trainer's own session list. All
// fixture data is invented.

import { expect, test } from './fixtures';

const PASSWORD = 'invented-passphrase-1';
const TRAINER_PASSWORD = 'trainer-passphrase-3';


const content = {
	name: 'Example County CTO Program',
	label: '2026 rev A',
	description: 'Invented program for session e2e.',
	phases: [
		{ name: 'Phase One', description: 'Observation.', presentation_number: 1 },
		{ name: 'Phase Two', description: 'Guided performance.', presentation_number: 2 }
	],
	phase_transitions: [
		{ from_phase: 'Phase One', to_phase: 'Phase Two', kind: 'advance' }
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

test('record, close, and read sessions with local time semantics', async ({ page, setupCode }) => {
	// Initialize and sign in as the administrator.
	await page.goto(`/`);
	await expect(page).toHaveURL(/\/setup$/);
	await page.getByLabel('Setup code').fill(setupCode);
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

	// Seed the published program, trainee, trainer, enrollment, and
	// assignment over the API; the session cookie rides page.request.
	const program = await (
		await page.request.post(`/api/programs`, { data: { name: content.name } })
	).json();
	const version = await (
		await page.request.post(`/api/programs/${program.id}/versions`, { data: content })
	).json();
	await page.request.post(`/api/program-versions/${version.id}/publish`, { data: {} });
	const trainee = await (
		await page.request.post(`/api/users`, {
			data: { username: 'taylor.trainee', display_name: 'Taylor Trainee' }
		})
	).json();
	const trainer = await (
		await page.request.post(`/api/users`, {
			data: {
				username: 'jordan.trainer',
				display_name: 'Jordan Trainer',
				role: 'trainer'
			}
		})
	).json();
	const enrollment = await (
		await page.request.post(`/api/program-versions/${version.id}/enrollments`, {
			data: { user_id: trainee.id }
		})
	).json();
	await page.request.post(`/api/enrollments/${enrollment.id}/assignments`, {
		data: { trainer_user_id: trainer.id }
	});

	// Record a session for Jordan with agency-local times.
	await page.goto(`/enrollments/${enrollment.id}`);
	await expect(page.getByRole('heading', { name: 'Training sessions' })).toBeVisible();
	await expect(page.getByText('No sessions recorded.')).toBeVisible();
	await page.getByLabel('Business date').fill('2026-06-02');
	await page.getByLabel('Timezone').fill('America/Chicago');
	await page.getByLabel('Local start').fill('2026-06-02T07:00');
	await page.getByLabel('Trainer', { exact: true }).selectOption({ label: 'Jordan Trainer' });
	await page.getByRole('button', { name: 'Record session' }).click();
	await expect(page.getByText('Open', { exact: true })).toBeVisible();
	await expect(page.getByText('(America/Chicago)')).toBeVisible();
	const sessionsPanel = page.locator('section.panel', { hasText: 'Training sessions' });
	await expect(
		sessionsPanel.locator('span.trainer', { hasText: 'Jordan Trainer' })
	).toBeVisible();

	// A second session in the open window is refused by the invariant.
	await page.getByLabel('Business date').fill('2026-06-02');
	await page.getByLabel('Local start').fill('2026-06-02T15:00');
	await page.getByLabel('Trainer', { exact: true }).selectOption({ label: 'Jordan Trainer' });
	await page.getByRole('button', { name: 'Record session' }).click();
	await expect(
		page.getByText('active training intervals for one trainee cannot overlap')
	).toBeVisible();

	// Close the open session as completed.
	await page.getByLabel('Local end', { exact: true }).fill('2026-06-02T15:00');
	await sessionsPanel.getByRole('button', { name: 'Complete', exact: true }).click();
	await expect(sessionsPanel.getByText('completed', { exact: true })).toBeVisible();

	// Seed a second, open session so the trainer can work it from home.
	await page.request.post(`/api/enrollments/${enrollment.id}/sessions`, {
		data: {
			business_date: '2026-06-03',
			timezone: 'America/Chicago',
			local_start: '2026-06-03T07:00',
			trainer_user_ids: [trainer.id]
		}
	});

	// The trainer signs in and finds the sessions on their own list.
	await page.getByRole('link', { name: 'Home' }).click();
	await page.getByRole('button', { name: 'Sign out' }).click();
	await page.goto(`/reset`);
	await page.getByLabel('Username').fill('jordan.trainer');
	await page.getByLabel('Reset code').fill(trainer.reset_code);
	await page.getByLabel('New password').fill(TRAINER_PASSWORD);
	await page.getByRole('button', { name: 'Set new password' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Username').fill('jordan.trainer');
	await page.getByLabel('Password').fill(TRAINER_PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();
	await expect(page.getByRole('heading', { name: 'My sessions' })).toBeVisible();
	await expect(page.getByRole('cell', { name: 'Taylor Trainee' }).first()).toBeVisible();
	await expect(page.getByText('2026-06-02 07:00 – 2026-06-02 15:00')).toBeVisible();

	// Membership authorizes the close right here: the trainer completes
	// their open session from their own list.
	await page.getByLabel('Local end').fill('2026-06-03T15:00');
	await page.getByRole('button', { name: 'Complete', exact: true }).click();
	await expect(page.getByText('2026-06-03 07:00 – 2026-06-03 15:00')).toBeVisible();
});
