<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		ApiError,
		addSessionTrainer,
		closeSession,
		createAssignment,
		createDraft,
		createSession,
		dailyForms,
		downloadExport,
		endAssignment,
		enrollmentExportPath,
		enrollmentPacketPath,
		getEnrollment,
		getProgramVersions,
		listSessions,
		listUsers,
		recordEnrollmentEvent,
		recordPhaseEvent,
		removeSessionTrainer,
		recordSignoff,
		signoffMatrix,
		createWeeklySummary,
		summaryForms,
		updateSession,
		type EnrollmentDetail,
		type SignoffKind,
		type SignoffTask,
		type EnrollmentEventKind,
		type PhaseEventKind,
		type PhaseRef,
		type SessionDisposition,
		type TrainingSession,
		type UserSummary,
		type VersionSummary
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from '../../+layout';

	let { data }: { data: ShellData } = $props();
	let enrollmentId = $derived(Number(page.params.id));
	let myUserId = $derived(data.session?.user.id ?? 0);
	let canAssign = $derived(
		data.session?.capabilities.includes('assign_training') ?? false
	);
	let canAuthor = $derived(
		data.session?.capabilities.includes('author_evaluation') ?? false
	);

	let detail: EnrollmentDetail | null = $state(null);
	let error = $state('');
	let busy = $state(false);

	// Every finalized version of this enrollment's records leaves as one
	// archive of the stored record bytes with manifests (ADR 0014).
	let exportError = $state('');
	let exported = $state('');
	async function exportArchive(path: string) {
		exportError = '';
		exported = '';
		busy = true;
		try {
			exported = await downloadExport(path);
		} catch (err) {
			exportError = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
	function exportEnrollment() {
		void exportArchive(enrollmentExportPath(enrollmentId));
	}
	// The packet is the trainee leaving with their records: the same
	// units plus acknowledgments, amendments, signoff history, and the
	// enrollment's own history (ADR 0015).
	function exportPacket() {
		void exportArchive(enrollmentPacketPath(enrollmentId));
	}

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

	// Weekly summaries: an ordinary record on this enrollment whose copy
	// links the exact finalized daily versions it covers (ADR 0013).
	let summaryError = $state('');
	let summaryFormChoices: { id: number; name: string }[] | null = $state(null);
	let summaryFormChoice: number | '' = $state('');

	async function startSummary(formId?: number) {
		summaryError = '';
		busy = true;
		try {
			if (formId === undefined) {
				const { forms } = await summaryForms(enrollmentId);
				if (forms.length > 1) {
					summaryFormChoices = forms;
					summaryFormChoice = forms[0].id;
					return;
				}
				formId = forms[0]?.id;
			}
			const created = await createWeeklySummary(enrollmentId, formId);
			await goto(`/drafts/${created.id}`);
		} catch (err) {
			summaryError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	// Task signoffs: versioned per task; overrides carry a reason.
	let signoffs: SignoffTask[] = $state([]);
	let signoffError = $state('');
	let overrideReason: Record<number, string> = $state({});

	async function reloadSignoffs() {
		try {
			signoffs = (await signoffMatrix(enrollmentId)).tasks;
		} catch {
			signoffs = [];
		}
	}

	$effect(() => {
		const current = detail;
		if (current !== null) {
			void reloadSignoffs();
		}
	});

	async function sign(task: SignoffTask, kind: SignoffKind) {
		signoffError = '';
		busy = true;
		try {
			await recordSignoff(
				enrollmentId,
				task.task_id,
				kind,
				task.history > 0 ? overrideReason[task.task_id] : undefined
			);
			overrideReason[task.task_id] = '';
			await reloadSignoffs();
		} catch (err) {
			signoffError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	function signoffLabel(task: SignoffTask): string {
		switch (task.kind) {
			case 'observed':
				return 'Observed';
			case 'demonstrated':
				return 'Demonstrated';
			case 'revoked':
				return 'Revoked';
			case null:
				return '—';
		}
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

	// ---------------------------------------------------------- sessions
	let sessions: TrainingSession[] = $state([]);
	let sessionError = $state('');
	let amAssigned = $derived.by(() => {
		const current = detail;
		return (
			current !== null &&
			current.assignments.some(
				(a) => a.ended_at === null && a.trainer_user_id === myUserId
			)
		);
	});
	// Recording a session takes assign_training, or authoring plus an
	// active assignment; working one takes coordination or membership.
	let canRecord = $derived(canAssign || (canAuthor && amAssigned));

	async function loadSessions() {
		try {
			sessions = (await listSessions(enrollmentId)).sessions;
		} catch {
			sessions = [];
		}
	}

	$effect(() => {
		if (detail !== null) {
			void loadSessions();
		}
	});

	function canWork(session: TrainingSession): boolean {
		return canAssign || session.trainers.some((t) => t.user_id === myUserId);
	}

	// A version may pin several daily forms; the picker appears exactly
	// when the choice is real.
	let draftForms: Record<number, { id: number; name: string }[]> = $state({});
	let draftFormChoice: Record<number, number> = $state({});

	async function startDraft(session: TrainingSession, formId?: number) {
		busy = true;
		error = '';
		try {
			if (formId === undefined) {
				const { forms } = await dailyForms(session.id);
				if (forms.length > 1) {
					draftForms[session.id] = forms;
					draftFormChoice[session.id] = forms[0].id;
					return;
				}
				formId = forms[0]?.id;
			}
			const created = await createDraft(session.id, formId);
			await goto(`/drafts/${created.id}`);
		} catch (err) {
			error = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	let newDate = $state('');
	let newTz = $state(Intl.DateTimeFormat().resolvedOptions().timeZone);
	let newStart = $state('');
	let newEnd = $state('');
	let newDisposition: 'completed' | 'interrupted' = $state('completed');
	let newPhaseId = $state(0);
	let newTrainerId = $state(0);
	let sessionTrainerChoices = $derived(
		roster.filter((person) => person.capabilities.includes('author_evaluation'))
	);

	async function recordSession(event: SubmitEvent) {
		event.preventDefault();
		sessionError = '';
		busy = true;
		try {
			await createSession(enrollmentId, {
				business_date: newDate,
				timezone: newTz,
				local_start: newStart,
				...(newEnd === ''
					? {}
					: { local_end: newEnd, disposition: newDisposition as SessionDisposition }),
				...(newPhaseId === 0 ? {} : { phase_id: newPhaseId }),
				trainer_user_ids: newTrainerId === 0 ? [] : [newTrainerId]
			});
			newDate = '';
			newStart = '';
			newEnd = '';
			newDisposition = 'completed';
			newPhaseId = 0;
			newTrainerId = 0;
			await loadSessions();
		} catch (err) {
			sessionError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	let closeEnd: Record<number, string> = $state({});
	async function closeOne(session: TrainingSession, disposition: SessionDisposition) {
		sessionError = '';
		busy = true;
		try {
			await closeSession(
				session.id,
				disposition,
				disposition === 'cancelled' ? undefined : closeEnd[session.id]
			);
			await loadSessions();
		} catch (err) {
			sessionError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	let editingId: number | null = $state(null);
	let editDate = $state('');
	let editTz = $state('');
	let editStart = $state('');
	function startEdit(session: TrainingSession) {
		editingId = session.id;
		editDate = session.business_date;
		editTz = session.timezone;
		editStart = session.local_start;
	}
	async function saveEdit(session: TrainingSession) {
		sessionError = '';
		busy = true;
		try {
			await updateSession(session.id, {
				business_date: editDate,
				timezone: editTz,
				local_start: editStart,
				...(session.phase_id === null ? {} : { phase_id: session.phase_id })
			});
			editingId = null;
			await loadSessions();
		} catch (err) {
			sessionError = actionFailed(err);
		} finally {
			busy = false;
		}
	}

	let memberChoice: Record<number, number> = $state({});
	function addableTrainers(session: TrainingSession): UserSummary[] {
		return sessionTrainerChoices.filter(
			(person) => !session.trainers.some((t) => t.user_id === person.id)
		);
	}
	async function addMember(session: TrainingSession) {
		sessionError = '';
		busy = true;
		try {
			await addSessionTrainer(session.id, memberChoice[session.id]);
			await loadSessions();
		} catch (err) {
			sessionError = actionFailed(err);
		} finally {
			busy = false;
		}
	}
	async function removeMember(session: TrainingSession, userId: number) {
		sessionError = '';
		busy = true;
		try {
			await removeSessionTrainer(session.id, userId);
			await loadSessions();
		} catch (err) {
			sessionError = actionFailed(err);
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
		<h2>Training sessions</h2>
		{#if sessions.length === 0}
			<p class="quiet">No sessions recorded.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Date</th>
						<th>Local time</th>
						<th>Phase</th>
						<th>Trainers</th>
						<th>Status</th>
						<th>Draft</th>
						{#if canRecord}
							<th></th>
						{/if}
					</tr>
				</thead>
				<tbody>
					{#each sessions as session (session.id)}
						<tr>
							<td>{session.business_date}</td>
							<td>
								{session.local_start.replace('T', ' ')}
								{#if session.local_end}
									– {session.local_end.replace('T', ' ')}
								{/if}
								<span class="quiet-inline">({session.timezone})</span>
							</td>
							<td>{session.phase_name ?? '—'}</td>
							<td>
								{#each session.trainers as trainer (trainer.user_id)}
									<span class="trainer">
										{trainer.display_name}
										{#if canAssign && session.trainers.length > 1}
											<button
												type="button"
												class="secondary small"
												aria-label={`Remove ${trainer.display_name}`}
												disabled={busy}
												onclick={() => removeMember(session, trainer.user_id)}
											>
												✕
											</button>
										{/if}
									</span>
								{/each}
								{#if canAssign && addableTrainers(session).length > 0}
									<span class="row add-member">
										<select
											aria-label="Trainer to add"
											bind:value={memberChoice[session.id]}
										>
											{#each addableTrainers(session) as person (person.id)}
												<option value={person.id}>{person.display_name}</option>
											{/each}
										</select>
										<button
											type="button"
											class="secondary small"
											disabled={busy || !memberChoice[session.id]}
											onclick={() => addMember(session)}
										>
											Add
										</button>
									</span>
								{/if}
							</td>
							<td>
								{#if session.disposition === null}
									<span class="pill draft">Open</span>
								{:else}
									{session.disposition}
								{/if}
							</td>
							<td>
								{#if session.draft_id !== null}
									<a href={`/drafts/${session.draft_id}`}>Open draft</a>
								{:else if session.disposition !== 'cancelled' && (canRecord || canWork(session))}
									{#if draftForms[session.id]}
										<span class="row add-member">
											<select
												aria-label="Daily form"
												bind:value={draftFormChoice[session.id]}
											>
												{#each draftForms[session.id] as form (form.id)}
													<option value={form.id}>{form.name}</option>
												{/each}
											</select>
											<button
												type="button"
												class="secondary small"
												disabled={busy}
												onclick={() =>
													startDraft(session, draftFormChoice[session.id])}
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
							{#if canRecord}
								<td>
									{#if session.disposition === null && canWork(session)}
										{#if editingId === session.id}
											<div class="sessionbar">
												<input
													aria-label="Edit business date"
													type="date"
													bind:value={editDate}
												/>
												<input aria-label="Edit timezone" bind:value={editTz} />
												<input
													aria-label="Edit local start"
													type="datetime-local"
													bind:value={editStart}
												/>
												<button
													type="button"
													class="small"
													disabled={busy}
													onclick={() => saveEdit(session)}
												>
													Save
												</button>
												<button
													type="button"
													class="secondary small"
													onclick={() => (editingId = null)}
												>
													Stop editing
												</button>
											</div>
										{:else}
											<div class="sessionbar">
												<input
													aria-label="Local end"
													type="datetime-local"
													bind:value={closeEnd[session.id]}
												/>
												<button
													type="button"
													class="small"
													disabled={busy || !closeEnd[session.id]}
													onclick={() => closeOne(session, 'completed')}
												>
													Complete
												</button>
												<button
													type="button"
													class="secondary small"
													disabled={busy || !closeEnd[session.id]}
													onclick={() => closeOne(session, 'interrupted')}
												>
													Interrupt
												</button>
												{#if session.draft_id === null}
													<button
														type="button"
														class="secondary small"
														disabled={busy}
														onclick={() => closeOne(session, 'cancelled')}
													>
														Cancel session
													</button>
												{/if}
												<button
													type="button"
													class="secondary small"
													onclick={() => startEdit(session)}
												>
													Edit
												</button>
											</div>
										{/if}
									{/if}
								</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if canRecord && detail.status === 'active'}
			<form class="session-form" onsubmit={recordSession}>
				<div class="row">
					<label class="inline" for="session-date">Business date</label>
					<input id="session-date" type="date" required bind:value={newDate} />
					<label class="inline" for="session-tz">Timezone</label>
					<input id="session-tz" required bind:value={newTz} />
				</div>
				<div class="row">
					<label class="inline" for="session-start">Local start</label>
					<input
						id="session-start"
						type="datetime-local"
						required
						bind:value={newStart}
					/>
					<label class="inline" for="session-end">Local end (retroactive)</label>
					<input id="session-end" type="datetime-local" bind:value={newEnd} />
				</div>
				<div class="row">
					{#if newEnd !== ''}
						<label class="inline" for="session-disposition">Disposition</label>
						<select id="session-disposition" bind:value={newDisposition}>
							<option value="completed">Completed</option>
							<option value="interrupted">Interrupted</option>
						</select>
					{/if}
					{#if detail.phases.length > 0}
						<label class="inline" for="session-phase">Phase</label>
						<select id="session-phase" bind:value={newPhaseId}>
							<option value={0}>None</option>
							{#each detail.phases as phase (phase.id)}
								<option value={phase.id}>{phase.name}</option>
							{/each}
						</select>
					{/if}
					{#if canAssign}
						<label class="inline" for="session-trainer">Trainer</label>
						<select id="session-trainer" bind:value={newTrainerId}>
							<option value={0} disabled={!canAuthor}>
								{canAuthor ? 'Myself' : 'Choose a trainer…'}
							</option>
							{#each sessionTrainerChoices as person (person.id)}
								<option value={person.id}>{person.display_name}</option>
							{/each}
						</select>
					{/if}
				</div>
				<button
					type="submit"
					disabled={busy || (canAssign && !canAuthor && newTrainerId === 0)}
				>
					Record session
				</button>
			</form>
		{/if}
		{#if sessionError}
			<p class="error" role="alert">{sessionError}</p>
		{/if}
	</section>

	<section class="panel">
		<h2>Export</h2>
		<p class="quiet">
			Every finalized version of this enrollment's records — superseded
			originals included — leaves as one archive of the stored record bytes
			with manifests. The trainee packet adds the acknowledgments,
			amendments, signoff history, and the enrollment's own history. Verify
			either anywhere with <code>consolebook-server export verify</code>.
		</p>
		<div class="row">
			<button type="button" class="secondary" disabled={busy} onclick={exportEnrollment}>
				Export finalized records
			</button>
			<button type="button" class="secondary" disabled={busy} onclick={exportPacket}>
				Download trainee packet
			</button>
			{#if exported}
				<span class="quiet" role="status">Downloaded {exported}.</span>
			{/if}
		</div>
		{#if exportError}
			<p class="error" role="alert">{exportError}</p>
		{/if}
	</section>

	{#if canAssign || canAuthor}
		<section class="panel">
			<h2>Weekly summaries</h2>
			<p class="quiet">
				A weekly summary is its own record: it carries authored narrative
				and links to the exact finalized daily reports it covers.
			</p>
			{#if summaryFormChoices !== null}
				<div class="row">
					<select aria-label="Weekly summary form" bind:value={summaryFormChoice}>
						{#each summaryFormChoices as form (form.id)}
							<option value={form.id}>{form.name}</option>
						{/each}
					</select>
					<button
						type="button"
						class="secondary"
						disabled={busy || summaryFormChoice === ''}
						onclick={() => startSummary(Number(summaryFormChoice))}
					>
						Create
					</button>
				</div>
			{:else}
				<button
					type="button"
					class="secondary"
					disabled={busy}
					onclick={() => startSummary()}
				>
					Start weekly summary
				</button>
			{/if}
			{#if summaryError}
				<p class="error" role="alert">{summaryError}</p>
			{/if}
		</section>

		<section class="panel">
			<h2>Task signoffs</h2>
			<p class="quiet">
				A signoff records that a configured task was observed or
				demonstrated. Changing recorded state is an override: it takes
				review authority and a reason, and the history stays.
			</p>
			{#if signoffs.length === 0}
				<p class="quiet">The pinned version defines no tasks.</p>
			{:else}
				<table class="grid">
					<thead>
						<tr>
							<th>Task</th>
							<th>State</th>
							<th></th>
						</tr>
					</thead>
					<tbody>
						{#each signoffs as task (task.task_id)}
							<tr>
								<td>
									<strong>{task.competency_name}</strong>
									<p class="quiet signoff-prompt">{task.prompt}</p>
								</td>
								<td>
									{signoffLabel(task)}
									{#if task.signed_by_display_name}
										<p class="quiet signoff-prompt">
											by {task.signed_by_display_name}
											{#if task.signed_at}
												{instant(task.signed_at)}
											{/if}
											{#if task.reason}
												— {task.reason}
											{/if}
										</p>
									{/if}
								</td>
								<td>
									<div class="row signoff-row">
										{#if task.history > 0}
											<input
												aria-label={`Override reason for ${task.prompt}`}
												placeholder="Override reason"
												bind:value={overrideReason[task.task_id]}
											/>
										{/if}
										<button
											type="button"
											class="secondary small"
											disabled={busy ||
												(task.history > 0 &&
													!(overrideReason[task.task_id] ?? '').trim())}
											onclick={() => sign(task, 'observed')}
										>
											Observed
										</button>
										<button
											type="button"
											class="secondary small"
											disabled={busy ||
												(task.history > 0 &&
													!(overrideReason[task.task_id] ?? '').trim())}
											onclick={() => sign(task, 'demonstrated')}
										>
											Demonstrated
										</button>
										{#if task.kind !== null && task.kind !== 'revoked'}
											<button
												type="button"
												class="secondary small"
												disabled={busy ||
													!(overrideReason[task.task_id] ?? '').trim()}
												onclick={() => sign(task, 'revoked')}
											>
												Revoke
											</button>
										{/if}
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
			{#if signoffError}
				<p class="error" role="alert">{signoffError}</p>
			{/if}
		</section>
	{/if}

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
	.session-form {
		margin-top: 1rem;
	}
	.session-form .row {
		margin-bottom: 1rem;
		flex-wrap: wrap;
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
	.trainer {
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
		margin-right: 0.5rem;
	}
	.add-member select {
		width: auto;
	}
	.signoff-prompt {
		margin: 0.15rem 0 0;
		font-size: 0.85rem;
	}
	.signoff-row {
		flex-wrap: wrap;
	}
	.signoff-row input {
		width: 12rem;
	}
</style>
