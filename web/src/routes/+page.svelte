<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import {
		ApiError,
		createUser,
		getHealth,
		getNotices,
		issueResetCode,
		logout,
		markNoticeRead,
		myAssignments,
		type AssignedTrainee,
		type CreatedUser,
		type Health,
		type Notice,
		type Role
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from './+layout';

	let { data }: { data: ShellData } = $props();

	// The layout guard redirects unauthenticated visitors, so session is
	// present here; the fallback satisfies the type system honestly.
	let session = $derived(data.session);
	let canManageUsers = $derived(
		data.session?.capabilities.includes('manage_users') ?? false
	);
	let canViewAssigned = $derived(
		data.session?.capabilities.includes('view_assigned_records') ?? false
	);

	let health: Health | null = $state(null);
	let notices: Notice[] = $state([]);
	let assigned: AssignedTrainee[] = $state([]);
	$effect(() => {
		getHealth().then(
			(h) => (health = h),
			() => (health = null)
		);
		getNotices().then(
			(body) => (notices = body.notices),
			() => (notices = [])
		);
		if (canViewAssigned) {
			myAssignments().then(
				(body) => (assigned = body.assignments),
				() => (assigned = [])
			);
		}
	});

	async function acknowledge(notice: Notice) {
		await markNoticeRead(notice.id);
		notices = (await getNotices()).notices;
		await invalidateAll();
	}

	let resetUsername = $state('');
	let issued: { username: string; reset_code: string; expires_at: number } | null =
		$state(null);
	let issueError = $state('');
	let busy = $state(false);

	async function signOut() {
		await logout();
		await invalidateAll();
		await goto('/login');
	}

	async function issueCode(event: SubmitEvent) {
		event.preventDefault();
		issueError = '';
		issued = null;
		busy = true;
		try {
			issued = await issueResetCode(resetUsername.trim());
		} catch (err) {
			issueError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	let newUsername = $state('');
	let newDisplayName = $state('');
	let newEmployeeId = $state('');
	let newTitle = $state('');
	let newRole: Role = $state('trainee');
	let created: CreatedUser | null = $state(null);
	let createError = $state('');

	async function addUser(event: SubmitEvent) {
		event.preventDefault();
		createError = '';
		created = null;
		busy = true;
		try {
			created = await createUser({
				username: newUsername,
				display_name: newDisplayName,
				employee_id: newEmployeeId,
				title: newTitle,
				role: newRole
			});
			newUsername = '';
			newDisplayName = '';
			newEmployeeId = '';
			newTitle = '';
			newRole = 'trainee';
		} catch (err) {
			createError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Installation status</h1>
<p class="lede">Signed in as {session?.user.display_name}.</p>

<section class="card">
	<dl class="facts">
		<dt>Agency</dt>
		<dd>{data.instance.agency}</dd>
		<dt>Signed in as</dt>
		<dd>{session?.user.username}</dd>
		<dt>Capabilities</dt>
		<dd>{session?.capabilities.join(', ') || 'none'}</dd>
		<dt>Session expires</dt>
		<dd>{session ? instant(session.expires_at) : '—'}</dd>
		<dt>Server version</dt>
		<dd>{data.instance.version}</dd>
		<dt>Database</dt>
		<dd>{health ? health.database : 'checking…'}</dd>
	</dl>
</section>

<section class="card">
	<h2>Notices</h2>
	{#if notices.length === 0}
		<p class="quiet">No notices.</p>
	{:else}
		<ul class="notices">
			{#each notices as notice (notice.id)}
				<li class:unread={notice.read_at === null}>
					<p>{notice.message}</p>
					<p class="meta">
						{instant(notice.created_at)}
						{#if notice.read_at === null}
							<button class="secondary small" onclick={() => acknowledge(notice)}>
								Mark read
							</button>
						{/if}
					</p>
				</li>
			{/each}
		</ul>
	{/if}
</section>

{#if canViewAssigned}
	<section class="card">
		<h2>My trainees</h2>
		{#if assigned.length === 0}
			<p class="quiet">No active training assignments.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Trainee</th>
						<th>Program</th>
						<th>Assigned</th>
					</tr>
				</thead>
				<tbody>
					{#each assigned as row (row.assignment_id)}
						<tr>
							<td>
								<a href={`/enrollments/${row.enrollment_id}`}>
									{row.trainee_display_name}
								</a>
							</td>
							<td>{row.program_name} — v{row.version_number}</td>
							<td>{instant(row.assigned_at)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
{/if}

{#if canManageUsers}
	<section class="card">
		<h2>Create a user</h2>
		<p>
			The account starts with the chosen role's capabilities and no password.
			Relay the one-time code below; their first sign-in sets a password
			through the reset page.
		</p>
		<form onsubmit={addUser}>
			<label for="new-username">New username</label>
			<input id="new-username" required bind:value={newUsername} />
			<label for="new-display-name">Display name</label>
			<input id="new-display-name" bind:value={newDisplayName} />
			<label for="new-role">Role</label>
			<select id="new-role" bind:value={newRole}>
				<option value="trainee">Trainee (no capabilities yet)</option>
				<option value="trainer">Trainer</option>
				<option value="coordinator">Coordinator</option>
				<option value="administrator">Administrator</option>
			</select>
			<label for="new-employee-id">Employee identifier</label>
			<input id="new-employee-id" bind:value={newEmployeeId} />
			<label for="new-title">Title</label>
			<input id="new-title" bind:value={newTitle} />
			{#if createError}
				<p class="error" role="alert">{createError}</p>
			{/if}
			{#if created}
				<p class="code-out">
					Created <strong>{created.username}</strong>. First sign-in code:
					<code>{created.reset_code}</code>
					(valid until {instant(created.reset_expires_at)})
				</p>
			{/if}
			<button type="submit" disabled={busy}>Create user</button>
		</form>
	</section>

	<section class="card">
		<h2>Issue a password reset code</h2>
		<p>
			The code is single-use, valid for 15 minutes, and shown only here — relay
			it to the user directly. Using it signs them out everywhere.
		</p>
		<form onsubmit={issueCode}>
			<label for="reset-username">Username</label>
			<input id="reset-username" required bind:value={resetUsername} />
			{#if issueError}
				<p class="error" role="alert">{issueError}</p>
			{/if}
			{#if issued}
				<p class="code-out">
					Reset code for <strong>{issued.username}</strong>:
					<code>{issued.reset_code}</code>
					(valid until {instant(issued.expires_at)})
				</p>
			{/if}
			<button type="submit" disabled={busy}>Issue reset code</button>
		</form>
	</section>
{/if}

<button class="secondary" onclick={signOut}>Sign out</button>

<style>
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.5rem;
	}
	.code-out {
		background: light-dark(#eef4ee, #1e2a1f);
		border: 1px solid light-dark(#bcd4bc, #3c553d);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
		font-size: 0.92rem;
		overflow-wrap: anywhere;
	}
	code {
		font-size: 1.05em;
	}
	.quiet {
		opacity: 0.7;
		margin: 0;
	}
	ul.notices {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	ul.notices li {
		border-top: 1px solid light-dark(#e2e5ea, #333a47);
		padding: 0.6rem 0;
	}
	ul.notices li:first-child {
		border-top: 0;
	}
	ul.notices li p {
		margin: 0 0 0.25rem;
	}
	ul.notices li.unread > p:first-child {
		font-weight: 600;
	}
	.meta {
		font-size: 0.82rem;
		opacity: 0.75;
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}
	button.small {
		padding: 0.2rem 0.6rem;
		font-size: 0.82rem;
	}
</style>
