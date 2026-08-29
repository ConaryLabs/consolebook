<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import {
		ApiError,
		closeSession,
		createDraft,
		createUser,
		dailyForms,
		getHealth,
		getNotices,
		issueResetCode,
		logout,
		markNoticeRead,
		myAssignments,
		mySessions,
		reviewQueue,
		type AssignedTrainee,
		type CreatedUser,
		type Health,
		type MySession,
		type Notice,
		type ReviewQueueRow,
		type Role,
		type SessionDisposition
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
	let canAuthor = $derived(
		data.session?.capabilities.includes('author_evaluation') ?? false
	);
	let canReview = $derived(
		data.session?.capabilities.includes('review_evaluation') ?? false
	);

	let health: Health | null = $state(null);
	let notices: Notice[] = $state([]);
	let assigned: AssignedTrainee[] = $state([]);
	let sessions: MySession[] = $state([]);
	let queue: ReviewQueueRow[] = $state([]);
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
		if (canAuthor) {
			mySessions().then(
				(body) => (sessions = body.sessions),
				() => (sessions = [])
			);
		}
		if (canReview) {
			reviewQueue().then(
				(body) => (queue = body.drafts),
				() => (queue = [])
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

	// Session members work their sessions from here even without a durable
	// assignment: membership is the grant.
	let sessionEnd: Record<number, string> = $state({});
	let sessionError = $state('');
	async function closeMine(session: MySession, disposition: SessionDisposition) {
		sessionError = '';
		busy = true;
		try {
			await closeSession(
				session.session_id,
				disposition,
				disposition === 'cancelled' ? undefined : sessionEnd[session.session_id]
			);
			sessions = (await mySessions()).sessions;
		} catch (err) {
			sessionError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	// A version may pin several daily forms; the picker appears exactly
	// when the choice is real.
	let draftForms: Record<number, { id: number; name: string }[]> = $state({});
	let draftFormChoice: Record<number, number> = $state({});

	async function startDraft(session: MySession, formId?: number) {
		sessionError = '';
		busy = true;
		try {
			if (formId === undefined) {
				const { forms } = await dailyForms(session.session_id);
				if (forms.length > 1) {
					draftForms[session.session_id] = forms;
					draftFormChoice[session.session_id] = forms[0].id;
					return;
				}
				formId = forms[0]?.id;
			}
			const created = await createDraft(session.session_id, formId);
			await goto(`/drafts/${created.id}`);
		} catch (err) {
			sessionError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
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

{#if canAuthor}
	<section class="card">
		<h2>My sessions</h2>
		{#if sessions.length === 0}
			<p class="quiet">No sessions yet.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Date</th>
						<th>Local time</th>
						<th>Trainee</th>
						<th>Program</th>
						<th>Status</th>
						<th>Draft</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each sessions as session (session.session_id)}
						<tr>
							<td>{session.business_date}</td>
							<td>
								{session.local_start.replace('T', ' ')}
								{#if session.local_end}
									– {session.local_end.replace('T', ' ')}
								{/if}
							</td>
							<td>{session.trainee_display_name}</td>
							<td>{session.program_name} — v{session.version_number}</td>
							<td>{session.disposition ?? 'open'}</td>
							<td>
								{#if session.draft_id !== null}
									<a href={`/drafts/${session.draft_id}`}>Open draft</a>
								{:else if session.disposition !== 'cancelled'}
									{#if draftForms[session.session_id]}
										<span class="sessionbar">
											<select
												aria-label="Daily form"
												bind:value={draftFormChoice[session.session_id]}
											>
												{#each draftForms[session.session_id] as form (form.id)}
													<option value={form.id}>{form.name}</option>
												{/each}
											</select>
											<button
												type="button"
												class="secondary small"
												disabled={busy}
												onclick={() =>
													startDraft(session, draftFormChoice[session.session_id])}
											>
												Create
											</button>
										</span>
									{:else}
										<button
											type="button"
											class="secondary small"
											disabled={busy}
											onclick={() => startDraft(session)}
										>
											Start draft
										</button>
									{/if}
								{:else}
									<span class="quiet-inline">—</span>
								{/if}
							</td>
							<td>
								{#if session.disposition === null}
									<div class="sessionbar">
										<input
											aria-label="Local end"
											type="datetime-local"
											bind:value={sessionEnd[session.session_id]}
										/>
										<button
											type="button"
											class="small"
											disabled={busy || !sessionEnd[session.session_id]}
											onclick={() => closeMine(session, 'completed')}
										>
											Complete
										</button>
										<button
											type="button"
											class="secondary small"
											disabled={busy || !sessionEnd[session.session_id]}
											onclick={() => closeMine(session, 'interrupted')}
										>
											Interrupt
										</button>
										{#if session.draft_id === null}
											<button
												type="button"
												class="secondary small"
												disabled={busy}
												onclick={() => closeMine(session, 'cancelled')}
											>
												Cancel session
											</button>
										{/if}
									</div>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if sessionError}
			<p class="error" role="alert">{sessionError}</p>
		{/if}
	</section>
{/if}

{#if canReview}
	<section class="card">
		<h2>Review queue</h2>
		{#if queue.length === 0}
			<p class="quiet">Nothing awaiting review.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Trainee</th>
						<th>Program</th>
						<th>Owner</th>
						<th>Submitted</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each queue as row (row.record_id)}
						<tr>
							<td>{row.trainee_display_name}</td>
							<td>{row.program_name} — v{row.version_number}</td>
							<td>{row.owner_display_name}</td>
							<td>{instant(row.submitted_at)}</td>
							<td>
								<a href={`/drafts/${row.record_id}`}>
									{row.eligible ? 'Review' : 'Open'}
								</a>
								{#if !row.eligible}
									<span class="quiet-inline">(you contributed)</span>
								{/if}
							</td>
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
	.sessionbar {
		display: flex;
		gap: 0.35rem;
		align-items: center;
		flex-wrap: wrap;
	}
	.sessionbar input {
		margin: 0;
		width: auto;
	}
</style>
