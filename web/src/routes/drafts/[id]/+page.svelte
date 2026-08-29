<script lang="ts">
	import { page } from '$app/state';
	import {
		ApiError,
		getDraft,
		saveDraftContent,
		submitDraft,
		transferDraft,
		type DraftContent,
		type DraftView,
		type SkeletonCompetency
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from '../../+layout';

	let { data }: { data: ShellData } = $props();
	let draftId = $derived(Number(page.params.id));
	let myUserId = $derived(data.session?.user.id ?? 0);
	let canAssign = $derived(
		data.session?.capabilities.includes('assign_training') ?? false
	);
	let canAuthor = $derived(
		data.session?.capabilities.includes('author_evaluation') ?? false
	);

	let view: DraftView | null = $state(null);
	let error = $state('');
	let busy = $state(false);

	// The working copy under edit, keyed by the pinned vocabulary ids.
	let values: Record<number, number | null> = $state({});
	let modifiers: Record<number, Record<number, boolean>> = $state({});
	let narratives: Record<number, string> = $state({});

	async function load() {
		try {
			const fetched = await getDraft(draftId);
			view = fetched;
			const nextValues: Record<number, number | null> = {};
			const nextModifiers: Record<number, Record<number, boolean>> = {};
			for (const competency of fetched.form.competencies) {
				nextValues[competency.form_competency_id] = null;
				nextModifiers[competency.form_competency_id] = {};
			}
			for (const rating of fetched.content.ratings) {
				nextValues[rating.form_competency_id] = rating.value;
				const picked: Record<number, boolean> = {};
				for (const id of rating.modifier_ids) {
					picked[id] = true;
				}
				nextModifiers[rating.form_competency_id] = picked;
			}
			const nextNarratives: Record<number, string> = {};
			for (const narrative of fetched.form.narratives) {
				nextNarratives[narrative.form_narrative_id] = '';
			}
			for (const entry of fetched.content.narratives) {
				nextNarratives[entry.form_narrative_id] = entry.text;
			}
			values = nextValues;
			modifiers = nextModifiers;
			narratives = nextNarratives;
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		}
	}

	$effect(() => {
		void load();
	});

	let editable = $derived.by(() => {
		const current = view;
		return current !== null && current.status === 'draft' && (canAssign || canAuthor);
	});
	let mayRoute = $derived.by(() => {
		const current = view;
		return current !== null && (canAssign || myUserId === current.owner_user_id);
	});

	function buildContent(): DraftContent {
		const current = view;
		if (current === null) {
			return { ratings: [], narratives: [] };
		}
		const ratings = [];
		for (const competency of current.form.competencies) {
			const id = competency.form_competency_id;
			const value = values[id] ?? null;
			const picked = Object.entries(modifiers[id] ?? {})
				.filter(([, on]) => on)
				.map(([modifierId]) => Number(modifierId));
			if (value !== null || picked.length > 0) {
				ratings.push({ form_competency_id: id, value, modifier_ids: picked });
			}
		}
		const texts = [];
		for (const narrative of current.form.narratives) {
			const id = narrative.form_narrative_id;
			const text = narratives[id] ?? '';
			if (text !== '') {
				texts.push({ form_narrative_id: id, text });
			}
		}
		return { ratings, narratives: texts };
	}

	// Autosave: debounced, with the save state visible so collaboration
	// never depends on a submit button.
	let saveState: 'idle' | 'pending' | 'saving' | 'saved' | 'failed' = $state('idle');
	let saveTimer: ReturnType<typeof setTimeout> | null = null;

	function scheduleSave() {
		if (!editable) {
			return;
		}
		saveState = 'pending';
		if (saveTimer !== null) {
			clearTimeout(saveTimer);
		}
		saveTimer = setTimeout(() => void saveNow(), 600);
	}

	async function saveNow() {
		saveTimer = null;
		saveState = 'saving';
		try {
			await saveDraftContent(draftId, buildContent());
			saveState = 'saved';
			await refreshMeta();
		} catch (err) {
			saveState = 'failed';
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		}
	}

	// Refresh attribution and workflow state without clobbering what the
	// contributor is typing.
	async function refreshMeta() {
		const current = view;
		if (current === null) {
			return;
		}
		try {
			const fetched = await getDraft(draftId);
			current.status = fetched.status;
			current.owner_user_id = fetched.owner_user_id;
			current.owner_display_name = fetched.owner_display_name;
			current.events = fetched.events;
			current.snapshots = fetched.snapshots;
			current.eligible_recipients = fetched.eligible_recipients;
		} catch {
			// The next save or reload surfaces the problem.
		}
	}

	let transferTo: number | '' = $state('');
	async function transfer() {
		if (transferTo === '') {
			return;
		}
		busy = true;
		error = '';
		try {
			await transferDraft(draftId, transferTo);
			transferTo = '';
			await refreshMeta();
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	async function submit() {
		busy = true;
		error = '';
		try {
			await submitDraft(draftId);
			await load();
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	function eventLine(kind: string): string {
		switch (kind) {
			case 'created':
				return 'created the draft';
			case 'contributed':
				return 'contributed';
			case 'ownership_transferred':
				return 'transferred ownership to';
			case 'submitted_for_review':
				return 'submitted for review';
			case 'review_decided':
				return 'decided the review';
			default:
				return kind;
		}
	}

	function anchorLabel(competency: SkeletonCompetency, value: number): string {
		const anchor = competency.anchors.find((candidate) => candidate.value === value);
		return anchor ? `${value} — ${anchor.label}` : String(value);
	}
</script>

<svelte:head>
	<title>Daily draft — Consolebook</title>
</svelte:head>

{#if view === null}
	{#if error}
		<p class="error" role="alert">{error}</p>
	{:else}
		<p class="quiet">Loading…</p>
	{/if}
{:else}
	<section class="panel">
		<div class="head">
			<div>
				<h1>{view.form.form_name}</h1>
				<p class="quiet">
					{view.trainee_display_name} · {view.program_name} — v{view.version_number}
					· owned by {view.owner_display_name}
				</p>
				{#each view.sessions as covered (covered.session_id)}
					<p class="quiet">
						Session {covered.business_date}:
						{covered.local_start.replace('T', ' ')}
						{#if covered.local_end}
							– {covered.local_end.replace('T', ' ')}
						{/if}
						<span class="quiet-inline">({covered.timezone})</span>
					</p>
				{/each}
			</div>
			<div class="workflow">
				{#if view.status === 'submitted'}
					<span class="pill submitted">Submitted for review</span>
				{:else}
					<span class="pill draft">Draft</span>
					<span class="savestate" role="status">
						{#if saveState === 'pending' || saveState === 'saving'}
							Saving…
						{:else if saveState === 'saved'}
							Saved
						{:else if saveState === 'failed'}
							Save failed
						{/if}
					</span>
				{/if}
			</div>
		</div>
		{#if view.form.instructions}
			<p class="instructions">{view.form.instructions}</p>
		{/if}
		{#if view.status === 'submitted'}
			<p class="quiet">
				The draft is frozen; its submitted content is snapshotted for review.
			</p>
		{/if}
		{#if error}
			<p class="error" role="alert">{error}</p>
		{/if}
	</section>

	<section class="panel">
		<h2>Ratings</h2>
		{#if view.form.competencies.length === 0}
			<p class="quiet">The pinned form defines no rated competencies.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Competency</th>
						<th>Scale</th>
						<th>Rating</th>
						{#if view.form.modifiers.length > 0}
							<th>Modifiers</th>
						{/if}
					</tr>
				</thead>
				<tbody>
					{#each view.form.competencies as competency (competency.form_competency_id)}
						<tr>
							<td>
								<strong>{competency.name}</strong>
								{#if competency.category}
									<span class="quiet-inline">({competency.category})</span>
								{/if}
								<p class="quiet small-note">{competency.description}</p>
							</td>
							<td>{competency.scale_name}</td>
							<td>
								{#if competency.scale_kind === 'narrative_only'}
									<span class="quiet-inline">narrative</span>
								{:else if competency.scale_kind === 'pass_fail'}
									<select
										aria-label={`Rate ${competency.name}`}
										disabled={!editable}
										bind:value={values[competency.form_competency_id]}
										onchange={scheduleSave}
									>
										<option value={null}>—</option>
										<option value={1}>Pass</option>
										<option value={0}>Fail</option>
									</select>
								{:else if competency.anchors.length > 0}
									<select
										aria-label={`Rate ${competency.name}`}
										disabled={!editable}
										bind:value={values[competency.form_competency_id]}
										onchange={scheduleSave}
									>
										<option value={null}>—</option>
										{#each competency.anchors as anchor (anchor.value)}
											<option value={anchor.value}>
												{anchorLabel(competency, anchor.value)}
											</option>
										{/each}
									</select>
								{:else}
									<input
										aria-label={`Rate ${competency.name}`}
										type="number"
										min={competency.min_value}
										max={competency.max_value}
										disabled={!editable}
										bind:value={values[competency.form_competency_id]}
										oninput={scheduleSave}
									/>
								{/if}
							</td>
							{#if view.form.modifiers.length > 0}
								<td>
									{#each view.form.modifiers as modifier (modifier.rating_modifier_id)}
										<label class="modifier" title={modifier.description}>
											<input
												type="checkbox"
												disabled={!editable}
												bind:checked={
													modifiers[competency.form_competency_id][
														modifier.rating_modifier_id
													]
												}
												onchange={scheduleSave}
											/>
											{modifier.code}
										</label>
									{/each}
								</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>

	<section class="panel">
		<h2>Narratives</h2>
		{#if view.form.narratives.length === 0}
			<p class="quiet">The pinned form defines no narrative prompts.</p>
		{:else}
			{#each view.form.narratives as narrative (narrative.form_narrative_id)}
				<div class="narrative">
					<label for={`narrative-${narrative.form_narrative_id}`}>
						{narrative.prompt}
						{#if narrative.required}
							<span class="required" title="Required before finalization">*</span>
						{/if}
					</label>
					<textarea
						id={`narrative-${narrative.form_narrative_id}`}
						rows="4"
						disabled={!editable}
						bind:value={narratives[narrative.form_narrative_id]}
						oninput={scheduleSave}
					></textarea>
				</div>
			{/each}
		{/if}
	</section>

	<section class="panel">
		<h2>Attribution</h2>
		<ul class="history">
			{#each view.events as event (event.id)}
				<li>
					<strong>{event.actor_display_name}</strong>
					{eventLine(event.kind)}
					{#if event.to_display_name}
						<strong>{event.to_display_name}</strong>
					{/if}
					<span class="quiet-inline">{instant(event.recorded_at)}</span>
				</li>
			{/each}
		</ul>
		{#if view.snapshots.length > 0}
			<p class="quiet">
				{view.snapshots.length}
				{view.snapshots.length === 1 ? 'snapshot' : 'snapshots'} on file.
			</p>
		{/if}
		{#if mayRoute && view.status === 'draft'}
			<div class="row route">
				{#if view.eligible_recipients.length > 0}
					<label class="inline" for="transfer-to">Transfer ownership</label>
					<select id="transfer-to" bind:value={transferTo}>
						<option value="">Choose a trainer…</option>
						{#each view.eligible_recipients as person (person.user_id)}
							<option value={person.user_id}>{person.display_name}</option>
						{/each}
					</select>
					<button
						type="button"
						class="secondary"
						disabled={busy || transferTo === ''}
						onclick={transfer}
					>
						Transfer
					</button>
				{/if}
				<button type="button" disabled={busy} onclick={submit}>
					Submit for review
				</button>
			</div>
		{/if}
	</section>
{/if}

<style>
	.panel {
		background: #fff;
		border: 1px solid #d8dee5;
		border-radius: 8px;
		padding: 1rem 1.25rem;
		margin-bottom: 1rem;
	}
	.head {
		display: flex;
		justify-content: space-between;
		gap: 1rem;
		align-items: flex-start;
	}
	.head h1 {
		margin: 0 0 0.25rem;
		font-size: 1.3rem;
	}
	.workflow {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		white-space: nowrap;
	}
	.pill {
		border-radius: 999px;
		padding: 0.15rem 0.6rem;
		font-size: 0.8rem;
		border: 1px solid #b9c2cc;
	}
	.pill.draft {
		background: #eef4fb;
	}
	.pill.submitted {
		background: #e8f6ec;
	}
	.savestate {
		font-size: 0.8rem;
		color: #5b6672;
		min-width: 5rem;
	}
	.instructions {
		white-space: pre-wrap;
	}
	table.grid {
		width: 100%;
		border-collapse: collapse;
	}
	table.grid th,
	table.grid td {
		text-align: left;
		padding: 0.45rem 0.6rem;
		border-bottom: 1px solid #e4e9ee;
		vertical-align: top;
	}
	.small-note {
		margin: 0.15rem 0 0;
		font-size: 0.85rem;
	}
	.modifier {
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
		margin-right: 0.6rem;
		font-size: 0.85rem;
	}
	.narrative {
		margin-bottom: 0.9rem;
	}
	.narrative label {
		display: block;
		margin-bottom: 0.3rem;
		font-weight: 600;
	}
	.narrative textarea {
		width: 100%;
		font: inherit;
		padding: 0.5rem;
	}
	.required {
		color: #a33;
	}
	.history {
		list-style: none;
		padding: 0;
		margin: 0 0 0.75rem;
	}
	.history li {
		padding: 0.2rem 0;
	}
	.row.route {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
	}
	.quiet {
		color: #5b6672;
	}
	.quiet-inline {
		color: #5b6672;
		font-size: 0.85rem;
	}
	.error {
		color: #a33;
	}
	label.inline {
		font-weight: 600;
	}
</style>
