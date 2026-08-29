<script lang="ts">
	import { page } from '$app/state';
	import {
		ApiError,
		getDraft,
		reviewDraft,
		saveDraftContent,
		submitDraft,
		transferDraft,
		type DraftContent,
		type DraftStatus,
		type DraftView,
		type ReviewDecisionKind,
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

	// The working copy under edit, keyed by the pinned vocabulary ids,
	// and the revision it was based on — every save carries it, so a
	// concurrent contributor's work is never silently overwritten.
	let revision = $state(0);
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
			revision = fetched.revision;
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		}
	}

	$effect(() => {
		void load();
	});

	// Draft, changes-requested, and returned states edit and resubmit;
	// submitted and approved states are frozen.
	function openStatus(status: DraftStatus): boolean {
		return status === 'draft' || status === 'changes_requested' || status === 'returned';
	}

	let editable = $derived.by(() => {
		const current = view;
		return current !== null && openStatus(current.status) && (canAssign || canAuthor);
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
	let inFlight: Promise<void> | null = null;
	let staleReloaded = false;

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

	// Saves are serialized into one chain: a new edit while a request is
	// in flight marks the chain dirty, and the chain re-sends the latest
	// state with the revision the previous save returned — overlapping
	// requests never race each other into a false conflict.
	let dirtyAgain = false;

	function saveNow(): Promise<void> {
		saveTimer = null;
		if (inFlight !== null) {
			dirtyAgain = true;
			return inFlight;
		}
		saveState = 'saving';
		const run = (async () => {
			try {
				// The metadata refresh stays inside the loop: an edit made
				// while it is awaited marks the chain dirty and re-runs the
				// save, so nothing typed during any await is dropped.
				do {
					dirtyAgain = false;
					saveState = 'saving';
					const saved = await saveDraftContent(draftId, revision, buildContent());
					revision = saved.revision;
					saveState = 'saved';
					await refreshMeta();
				} while (dirtyAgain);
			} catch (err) {
				if (err instanceof ApiError && err.code === 'stale_save') {
					// Another contributor saved first: their copy wins and
					// the page says so, rather than overwriting it.
					staleReloaded = true;
					await load();
					saveState = 'idle';
					error =
						'Another contributor saved first; the draft reloaded with their latest content.';
					return;
				}
				saveState = 'failed';
				error = err instanceof ApiError ? err.message : 'the server could not be reached';
			} finally {
				inFlight = null;
			}
		})();
		inFlight = run;
		return run;
	}

	// Nothing workflow-shaped runs over unsaved edits: a pending or
	// in-flight save lands first.
	async function flushSaves() {
		if (saveTimer !== null) {
			clearTimeout(saveTimer);
			await saveNow();
			return;
		}
		if (inFlight !== null) {
			await inFlight;
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
			await flushSaves();
			if (saveState === 'failed' || staleReloaded) {
				// A failed save or a reload from another contributor's copy
				// is not something to submit sight unseen.
				staleReloaded = false;
				return;
			}
			await submitDraft(draftId, revision);
			await load();
		} catch (err) {
			if (err instanceof ApiError && err.code === 'stale_save') {
				// The draft moved on since this page last saw it; show the
				// winning copy instead of freezing it sight unseen.
				await load();
				error =
					'Another contributor saved first; review the reloaded draft before submitting.';
				return;
			}
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	// The reviewer's decision, sent with its comment; the workspace
	// reloads onto the decided state.
	let reviewChoice: ReviewDecisionKind = $state('approved');
	let reviewComment = $state('');
	async function decide() {
		busy = true;
		error = '';
		try {
			await reviewDraft(draftId, reviewChoice, reviewComment.trim() || undefined);
			reviewComment = '';
			await load();
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	function statusLabel(status: DraftStatus): string {
		switch (status) {
			case 'draft':
				return 'Draft';
			case 'submitted':
				return 'Submitted for review';
			case 'changes_requested':
				return 'Changes requested';
			case 'returned':
				return 'Returned';
			case 'approved':
				return 'Approved';
		}
	}

	function decisionLabel(decision: ReviewDecisionKind): string {
		switch (decision) {
			case 'approved':
				return 'approved the draft';
			case 'changes_requested':
				return 'requested changes';
			case 'returned':
				return 'returned the draft';
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

	// A select enumerates the scale only while that stays usable; a wider
	// configured span gets a bounded numeric input instead of thousands
	// of options.
	const RANGE_SELECT_LIMIT = 24;

	function wideScale(competency: SkeletonCompetency): boolean {
		return (
			competency.min_value !== null &&
			competency.max_value !== null &&
			competency.max_value - competency.min_value > RANGE_SELECT_LIMIT
		);
	}

	// Every value the pinned scale accepts, not just the anchored ones —
	// anchors may be sparse (say, 1, 4, and 7 of a 1–7 scale) and label
	// the values they define.
	function numericValues(competency: SkeletonCompetency): number[] {
		if (
			competency.min_value === null ||
			competency.max_value === null ||
			wideScale(competency)
		) {
			return competency.anchors.map((anchor) => anchor.value);
		}
		const range: number[] = [];
		for (let value = competency.min_value; value <= competency.max_value; value += 1) {
			range.push(value);
		}
		return range;
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
				<span
					class="pill"
					class:submitted={view.status === 'submitted' || view.status === 'approved'}
					class:draft={openStatus(view.status)}
				>
					{statusLabel(view.status)}
				</span>
				{#if openStatus(view.status)}
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
		{:else if view.status === 'approved'}
			<p class="quiet">The draft is approved and stays frozen until finalization.</p>
		{:else if view.status === 'changes_requested' && view.decisions.length > 0}
			<p class="callout">
				Change request: {view.decisions[view.decisions.length - 1].comment}
			</p>
		{/if}
		{#if error}
			<p class="error" role="alert">{error}</p>
		{/if}
	</section>

	{#if view.viewer_may_review}
		<section class="panel">
			<h2>Review</h2>
			<label for="review-comment">
				Comment <span class="quiet-inline">(required when requesting changes)</span>
			</label>
			<textarea id="review-comment" rows="3" bind:value={reviewComment}></textarea>
			<div class="row route">
				<label class="inline" for="review-decision">Decision</label>
				<select id="review-decision" bind:value={reviewChoice}>
					<option value="approved">Approve</option>
					<option value="changes_requested">Request changes</option>
					<option value="returned">Return</option>
				</select>
				<button
					type="button"
					disabled={busy ||
						(reviewChoice === 'changes_requested' && reviewComment.trim() === '')}
					onclick={decide}
				>
					Decide
				</button>
			</div>
		</section>
	{/if}

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
							<td>
								{competency.scale_name}
								{#if competency.anchors.length > 0}
									<details class="anchors">
										<summary>Scale guide</summary>
										<ul>
											{#each competency.anchors as anchor (anchor.value)}
												<li>
													<strong>
														{competency.scale_kind === 'pass_fail'
															? anchor.label
															: anchorLabel(competency, anchor.value)}
													</strong>: {anchor.definition}
												</li>
											{/each}
										</ul>
									</details>
								{/if}
							</td>
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
										{#each competency.anchors as anchor (anchor.value)}
											<option value={anchor.value}>{anchor.label}</option>
										{/each}
									</select>
								{:else if wideScale(competency)}
									<input
										aria-label={`Rate ${competency.name}`}
										type="number"
										min={competency.min_value}
										max={competency.max_value}
										disabled={!editable}
										bind:value={values[competency.form_competency_id]}
										oninput={scheduleSave}
									/>
								{:else}
									<select
										aria-label={`Rate ${competency.name}`}
										disabled={!editable}
										bind:value={values[competency.form_competency_id]}
										onchange={scheduleSave}
									>
										<option value={null}>—</option>
										{#each numericValues(competency) as value (value)}
											<option {value}>{anchorLabel(competency, value)}</option>
										{/each}
									</select>
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
		{#if view.decisions.length > 0}
			<h3>Review decisions</h3>
			<ul class="history">
				{#each view.decisions as decision (decision.id)}
					<li>
						<strong>{decision.reviewer_display_name}</strong>
						{decisionLabel(decision.decision)}
						<span class="quiet-inline">{instant(decision.decided_at)}</span>
						{#if decision.comment}
							<p class="decision-comment">{decision.comment}</p>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if view.snapshots.length > 0}
			<p class="quiet">
				{view.snapshots.length}
				{view.snapshots.length === 1 ? 'snapshot' : 'snapshots'} on file.
			</p>
		{/if}
		{#if mayRoute && openStatus(view.status)}
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
	details.anchors {
		margin-top: 0.25rem;
		font-size: 0.85rem;
	}
	details.anchors summary {
		cursor: pointer;
		color: #5b6672;
	}
	details.anchors ul {
		margin: 0.25rem 0 0;
		padding-left: 1.1rem;
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
	.decision-comment {
		margin: 0.2rem 0 0.3rem;
		padding: 0.35rem 0.6rem;
		background: #f4f6f8;
		border-left: 3px solid #b9c2cc;
		white-space: pre-wrap;
	}
	.callout {
		padding: 0.45rem 0.6rem;
		background: #fdf3e3;
		border-left: 3px solid #d9a441;
		white-space: pre-wrap;
	}
	#review-comment {
		width: 100%;
		font: inherit;
		padding: 0.5rem;
		margin-bottom: 0.5rem;
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
