<script lang="ts">
	import { page } from '$app/state';
	import {
		ApiError,
		createAssignment,
		endAssignment,
		getEnrollment,
		getProgramVersions,
		listUsers,
		recordEnrollmentEvent,
		recordPhaseEvent,
		type EnrollmentDetail,
		type EnrollmentEventKind,
		type PhaseEventKind,
		type PhaseRef,
		type UserSummary,
		type VersionSummary
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from '../../+layout';

	let { data }: { data: ShellData } = $props();
	let enrollmentId = $derived(Number(page.params.id));
	let canAssign = $derived(
		data.session?.capabilities.includes('assign_training') ?? false
	);

	let detail: EnrollmentDetail | null = $state(null);
	let error = $state('');
	let busy = $state(false);

	async function reload() {
		try {
			detail = await getEnrollment(enrollmentId);
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		}
	}

	$effect(() => {
		void reload();
	});

	// Coordinator working data: the roster for assignment and the
	// program's published versions for a version change.
	let roster: UserSummary[] = $state([]);
	let versions: VersionSummary[] = $state([]);
	$effect(() => {
		const current = detail;
		if (!canAssign || current === null) {
			return;
		}
		listUsers().then(
			(body) => (roster = body.users),
			() => (roster = [])
		);
		getProgramVersions(current.program_id).then(
			(body) => (versions = body.versions),
			() => (versions = [])
		);
	});

	function actionFailed(err: unknown): string {
		return err instanceof ApiError ? err.message : 'the server could not be reached';
	}

	function kindLabel(kind: string): string {
		return kind.replaceAll('_', ' ');
	}

	// ------------------------------------------------------- assignments
	let assignUserId = $state(0);
	let assignError = $state('');
	// Only view_assigned_records holders are assignable (the server
	// refuses anyone else), minus trainers already actively assigned.
	let assignable = $derived.by(() => {
		const current = detail;
		if (current === null) {
			return [];
		}
		return roster.filter(
			(person) =>
				person.capabilities.includes('view_assigned_records') &&
				!current.assignments.some(
					(a) => a.ended_at === null && a.trainer_user_id === person.id
				)
		);
	});

	async function assign(event: SubmitEvent) {
		event.preventDefault();
		assignError = '';
		busy = true;
		try {
			await createAssignment(enrollmentId, assignUserId);
			assignUserId = 0;
			await reload();
		} catch (err) {
			assignError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	async function endOne(assignmentId: number) {
		assignError = '';
		busy = true;
		try {
			await endAssignment(assignmentId);
			await reload();
		} catch (err) {
			assignError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	// --------------------------------------------------------- lifecycle
	let lifecycleReason = $state('');
	let lifecycleError = $state('');
	let changeTargetId = $state(0);
	let changeable = $derived.by(() => {
		const current = detail;
		if (current === null) {
			return [];
		}
		return versions.filter(
			(v) => v.published_at !== null && v.id !== current.program_version_id
		);
	});

	async function lifecycleEvent(kind: EnrollmentEventKind, toVersionId?: number) {
		lifecycleError = '';
		busy = true;
		try {
			await recordEnrollmentEvent(enrollmentId, {
				kind,
				reason: lifecycleReason,
				...(toVersionId === undefined ? {} : { to_version_id: toVersionId })
			});
			lifecycleReason = '';
			changeTargetId = 0;
			await reload();
		} catch (err) {
			lifecycleError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	// ------------------------------------------------------------ phases
	let phaseKind: PhaseEventKind = $state('advance');
	let phaseTargetId = $state(0);
	let phaseReason = $state('');
	let phaseEffective = $state('');
	let phaseError = $state('');
	let needsTarget = $derived(
		phaseKind === 'advance' || phaseKind === 'return' || phaseKind === 'restart'
	);
	let phaseTargets: PhaseRef[] = $derived.by(() => {
		const current = detail;
		if (current === null || !needsTarget) {
			return [];
		}
		// Entry: an advance before any phase may target any phase of the
		// pinned version. Afterwards, targets follow the transition graph.
		if (current.current_phase_id === null) {
			return phaseKind === 'advance' ? current.phases : [];
		}
		const edgeKinds =
			phaseKind === 'advance'
				? ['advance', 'skip']
				: phaseKind === 'return'
					? ['remediation']
					: ['restart'];
		const targets = current.transitions
			.filter(
				(t) =>
					t.from_phase_id === current.current_phase_id && edgeKinds.includes(t.kind)
			)
			.map((t) => current.phases.find((p) => p.id === t.to_phase_id))
			.filter((p): p is PhaseRef => p !== undefined);
		return targets;
	});

	async function submitPhase(event: SubmitEvent) {
		event.preventDefault();
		phaseError = '';
		busy = true;
		try {
			await recordPhaseEvent(enrollmentId, {
				kind: phaseKind,
				...(needsTarget ? { to_phase_id: phaseTargetId } : {}),
				...(phaseEffective === ''
					? {}
					: { effective_at: Math.floor(new Date(phaseEffective).getTime() / 1000) }),
				reason: phaseReason
			});
			phaseTargetId = 0;
			phaseReason = '';
			phaseEffective = '';
			await reload();
		} catch (err) {
			phaseError = actionFailed(err);
		} finally {
			busy = false;
		}
	}
</script>

{#if detail !== null}
	<h1>{detail.trainee_display_name}</h1>
	<p class="lede">
		<span class={`pill status-${detail.status}`}>{kindLabel(detail.status)}</span>
		{#if detail.paused}
			<span class="pill paused">Paused</span>
		{/if}
		{detail.program_name} — v{detail.version_number}
		{#if detail.version_label}({detail.version_label}){/if}
		· enrolled {instant(detail.enrolled_at)}
		{#if detail.current_phase_name}
			· current phase: {detail.current_phase_name}
		{:else if detail.phases.length > 0}
			· no phase entered yet
		{/if}
	</p>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}

	<section class="panel">
		<h2>Assignments</h2>
		{#if detail.assignments.length === 0}
			<p class="quiet">No trainers are assigned to this enrollment.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Trainer</th>
						<th>Assigned</th>
						<th>Ended</th>
						{#if canAssign}
							<th></th>
						{/if}
					</tr>
				</thead>
				<tbody>
					{#each detail.assignments as assignment (assignment.id)}
						<tr>
							<td>{assignment.trainer_display_name}</td>
							<td>{instant(assignment.assigned_at)}</td>
							<td>
								{assignment.ended_at === null ? 'active' : instant(assignment.ended_at)}
							</td>
							{#if canAssign}
								<td>
									{#if assignment.ended_at === null}
										<button
											type="button"
											class="secondary small"
											disabled={busy}
											onclick={() => endOne(assignment.id)}
										>
											End
										</button>
									{/if}
								</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if canAssign && detail.status === 'active'}
			<form class="row actions" onsubmit={assign}>
				<select aria-label="Trainer to assign" bind:value={assignUserId} required>
					<option value={0} disabled>Choose a trainer…</option>
					{#each assignable as person (person.id)}
						<option value={person.id}>{person.display_name} ({person.username})</option>
					{/each}
				</select>
				<button type="submit" disabled={busy || assignUserId === 0}>Assign</button>
			</form>
		{/if}
		{#if assignError}
			<p class="error" role="alert">{assignError}</p>
		{/if}
	</section>

	<section class="panel">
		<h2>Phase history</h2>
		{#if detail.phases.length === 0}
			<p class="quiet">
				The pinned version defines no phases; this program has no progression.
			</p>
		{:else}
			{#if detail.phase_events.length === 0}
				<p class="quiet">No phase events yet.</p>
			{:else}
				<table class="grid">
					<thead>
						<tr>
							<th>Effective</th>
							<th>Event</th>
							<th>Phases</th>
							<th>Recorded by</th>
							<th>Reason</th>
						</tr>
					</thead>
					<tbody>
						{#each detail.phase_events as event (event.id)}
							<tr>
								<td>
									{instant(event.effective_at)}
									{#if event.recorded_at !== event.effective_at}
										<span class="quiet-inline">
											(recorded {instant(event.recorded_at)})
										</span>
									{/if}
								</td>
								<td>{kindLabel(event.kind)}</td>
								<td>
									{#if event.to_phase_name !== null}
										{event.from_phase_name ?? 'entry'} → {event.to_phase_name}
									{:else}
										{event.from_phase_name}
									{/if}
								</td>
								<td>{event.actor_display_name ?? '—'}</td>
								<td>{event.reason}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
			{#if canAssign && detail.status === 'active'}
				<form class="phase-form" onsubmit={submitPhase}>
					<div class="row">
						<label class="inline" for="phase-kind">Phase action</label>
						<select id="phase-kind" bind:value={phaseKind}>
							<option value="advance">Advance</option>
							<option value="return">Return for remediation</option>
							<option value="restart">Restart</option>
							<option value="pause">Pause</option>
							<option value="resume">Resume</option>
							<option value="complete">Complete</option>
						</select>
						{#if needsTarget}
							<label class="inline" for="phase-target">Target phase</label>
							<select id="phase-target" bind:value={phaseTargetId} required>
								<option value={0} disabled>Choose a phase…</option>
								{#each phaseTargets as phase (phase.id)}
									<option value={phase.id}>{phase.name}</option>
								{/each}
							</select>
						{/if}
					</div>
					<label for="phase-effective">Effective (local time; blank means now)</label>
					<input id="phase-effective" type="datetime-local" bind:value={phaseEffective} />
					<label for="phase-reason">Phase reason</label>
					<input
						id="phase-reason"
						bind:value={phaseReason}
						placeholder="Required for return and restart"
					/>
					<button type="submit" disabled={busy || (needsTarget && phaseTargetId === 0)}>
						Record phase event
					</button>
				</form>
			{/if}
			{#if phaseError}
				<p class="error" role="alert">{phaseError}</p>
			{/if}
		{/if}
	</section>

	<section class="panel">
		<h2>Enrollment lifecycle</h2>
		{#if detail.events.length === 0}
			<p class="quiet">No lifecycle events yet.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>When</th>
						<th>Event</th>
						<th>Detail</th>
						<th>Recorded by</th>
						<th>Reason</th>
					</tr>
				</thead>
				<tbody>
					{#each detail.events as event (event.id)}
						<tr>
							<td>{instant(event.occurred_at)}</td>
							<td>{kindLabel(event.kind)}</td>
							<td>
								{#if event.kind === 'version_change'}
									v{event.from_version_number} → v{event.to_version_number}
									{#if event.to_version_label}({event.to_version_label}){/if}
								{/if}
							</td>
							<td>{event.actor_display_name ?? '—'}</td>
							<td>{event.reason}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if canAssign}
			<div class="lifecycle-actions">
				<label for="lifecycle-reason">Lifecycle reason</label>
				<input
					id="lifecycle-reason"
					bind:value={lifecycleReason}
					placeholder="Required except for complete"
				/>
				<div class="row">
					{#if detail.status === 'active'}
						<button
							type="button"
							disabled={busy}
							onclick={() => lifecycleEvent('withdraw')}
						>
							Withdraw
						</button>
						<button
							type="button"
							disabled={busy}
							onclick={() => lifecycleEvent('complete')}
						>
							Complete
						</button>
					{:else}
						<button
							type="button"
							disabled={busy}
							onclick={() => lifecycleEvent('reinstate')}
						>
							Reinstate
						</button>
					{/if}
				</div>
				{#if detail.status === 'active' && changeable.length > 0}
					<div class="row">
						<select aria-label="New program version" bind:value={changeTargetId}>
							<option value={0} disabled>Change to version…</option>
							{#each changeable as version (version.id)}
								<option value={version.id}>
									v{version.version_number}
									{version.label ? `(${version.label})` : ''}
								</option>
							{/each}
						</select>
						<button
							type="button"
							disabled={busy || changeTargetId === 0}
							onclick={() => lifecycleEvent('version_change', changeTargetId)}
						>
							Change version
						</button>
					</div>
				{/if}
			</div>
		{/if}
		{#if lifecycleError}
			<p class="error" role="alert">{lifecycleError}</p>
		{/if}
	</section>

	<p>
		<a href={`/programs/${detail.program_id}/versions/${detail.program_version_id}`}>
			Pinned program version
		</a>
	</p>
{:else if error}
	<p class="error" role="alert">{error}</p>
{:else}
	<p>Loading…</p>
{/if}

<style>
	.quiet {
		opacity: 0.7;
		margin: 0 0 0.75rem;
	}
	.quiet-inline {
		opacity: 0.7;
		font-size: 0.85em;
	}
	.pill.status-active {
		background: light-dark(#dcefdd, #1e3524);
		color: light-dark(#1e5c28, #9fd3a8);
	}
	.pill.status-withdrawn {
		background: light-dark(#fbe9e9, #3a1d20);
		color: light-dark(#8c2f36, #e0a2a8);
	}
	.pill.status-completed {
		background: light-dark(#e2e9f5, #1f2a3d);
		color: light-dark(#2456a6, #9fbcf0);
	}
	.pill.paused {
		background: light-dark(#fdf1d7, #3b301e);
		color: light-dark(#7a5410, #e4c26d);
	}
	form.actions {
		margin-top: 1rem;
		max-width: 28rem;
	}
	.phase-form {
		margin-top: 1rem;
		max-width: 34rem;
	}
	.phase-form .row {
		margin-bottom: 1rem;
	}
	label.inline {
		margin: 0;
		white-space: nowrap;
	}
	.lifecycle-actions {
		margin-top: 1rem;
		max-width: 34rem;
	}
</style>
