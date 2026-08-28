<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		ApiError,
		discardVersion,
		enrollUser,
		getVersion,
		listEnrollments,
		listUsers,
		publishVersion,
		replaceVersionContent,
		versionExportPath,
		type Enrollee,
		type UserSummary,
		type VersionContent,
		type VersionSummary
	} from '$lib/api';
	import CitationsEditor from '$lib/editor/CitationsEditor.svelte';
	import CompetenciesEditor from '$lib/editor/CompetenciesEditor.svelte';
	import FormsEditor from '$lib/editor/FormsEditor.svelte';
	import PhasesEditor from '$lib/editor/PhasesEditor.svelte';
	import ScalesEditor from '$lib/editor/ScalesEditor.svelte';
	import { instant } from '$lib/format';
	import type { ShellData } from '../../../../+layout';

	let { data }: { data: ShellData } = $props();
	let canManage = $derived(data.session?.capabilities.includes('manage_programs') ?? false);
	let programId = $derived(Number(page.params.id));
	let versionId = $derived(Number(page.params.vid));

	let summary: VersionSummary | null = $state(null);
	let content: VersionContent | null = $state(null);
	let error = $state('');
	let problems: string[] = $state([]);
	let saved = $state('');
	let busy = $state(false);

	let published = $derived.by(() => {
		const current = summary;
		return current !== null && current.published_at !== null;
	});
	let disabled = $derived(published || !canManage);
	let competencyNames = $derived.by(() => {
		const current = content;
		return current === null ? [] : current.competencies.map((c) => c.name);
	});
	let scaleNames = $derived.by(() => {
		const current = content;
		return current === null ? [] : current.rating_scales.map((s) => s.name);
	});

	async function reload() {
		try {
			const body = await getVersion(versionId);
			summary = body.summary;
			content = body.content;
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		}
	}

	$effect(() => {
		void reload();
	});

	function failed(err: unknown) {
		if (err instanceof ApiError) {
			error = err.message;
			problems = err.problems;
		} else {
			error = 'the server could not be reached';
			problems = [];
		}
	}

	async function save() {
		if (content === null) {
			return;
		}
		error = '';
		problems = [];
		saved = '';
		busy = true;
		try {
			await replaceVersionContent(versionId, $state.snapshot(content));
			saved = 'Draft saved.';
			await reload();
		} catch (err) {
			failed(err);
		} finally {
			busy = false;
		}
	}

	async function publish() {
		if (
			!window.confirm(
				'Publish this version? Published versions are permanently immutable; corrections require a new version.'
			)
		) {
			return;
		}
		error = '';
		problems = [];
		saved = '';
		busy = true;
		try {
			if (content !== null) {
				await replaceVersionContent(versionId, $state.snapshot(content));
			}
			await publishVersion(versionId);
			await reload();
		} catch (err) {
			failed(err);
		} finally {
			busy = false;
		}
	}

	async function discard() {
		if (!window.confirm('Discard this draft? Its content is deleted.')) {
			return;
		}
		busy = true;
		try {
			await discardVersion(versionId);
			await goto(`/programs/${programId}`);
		} catch (err) {
			failed(err);
		} finally {
			busy = false;
		}
	}

	// Enrollment: published versions only, assign_training only.
	let canAssign = $derived(
		data.session?.capabilities.includes('assign_training') ?? false
	);
	let enrollees: Enrollee[] = $state([]);
	let roster: UserSummary[] = $state([]);
	let selectedUserId = $state(0);
	let enrollError = $state('');

	async function loadEnrollment() {
		const [enrolled, people] = await Promise.all([listEnrollments(versionId), listUsers()]);
		enrollees = enrolled.enrollees;
		roster = people.users;
	}

	$effect(() => {
		if (published && canAssign) {
			loadEnrollment().catch(() => {
				enrollees = [];
				roster = [];
			});
		}
	});

	let enrollable = $derived(
		roster.filter((person) => !enrollees.some((e) => e.user_id === person.id))
	);

	async function enroll(event: SubmitEvent) {
		event.preventDefault();
		enrollError = '';
		busy = true;
		try {
			await enrollUser(versionId, selectedUserId);
			selectedUserId = 0;
			await loadEnrollment();
		} catch (err) {
			enrollError = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

{#if summary !== null && content !== null}
	<h1>
		{content.name} — v{summary.version_number}
	</h1>
	<p class="lede">
		{#if published && summary.published_at !== null}
			<span class="pill published">Published {instant(summary.published_at)}</span>
			This version is immutable. To change it, create a new draft from it.
		{:else}
			<span class="pill draft">Draft</span>
			Editing replaces the whole draft; the last save wins.
		{/if}
	</p>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}
	{#if problems.length > 0}
		<ul class="problems" role="alert">
			{#each problems as problem}
				<li>{problem}</li>
			{/each}
		</ul>
	{/if}
	{#if saved}
		<p class="saved" role="status">{saved}</p>
	{/if}

	{#if published && canAssign}
		<section class="panel">
			<h2>Enrollments</h2>
			{#if enrollees.length === 0}
				<p class="quiet">Nobody is enrolled in this version yet.</p>
			{:else}
				<table class="grid">
					<thead>
						<tr>
							<th>Trainee</th>
							<th>Username</th>
							<th>Enrolled</th>
						</tr>
					</thead>
					<tbody>
						{#each enrollees as enrollee (enrollee.enrollment_id)}
							<tr>
								<td>
									<a href={`/enrollments/${enrollee.enrollment_id}`}>
										{enrollee.display_name}
									</a>
								</td>
								<td>{enrollee.username}</td>
								<td>{instant(enrollee.enrolled_at)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
			<form class="row enroll" onsubmit={enroll}>
				<select aria-label="Trainee to enroll" bind:value={selectedUserId} required>
					<option value={0} disabled>Choose a trainee…</option>
					{#each enrollable as person (person.id)}
						<option value={person.id}>{person.display_name} ({person.username})</option>
					{/each}
				</select>
				<button type="submit" disabled={busy || selectedUserId === 0}>Enroll</button>
			</form>
			{#if enrollError}
				<p class="error" role="alert">{enrollError}</p>
			{/if}
		</section>
	{/if}

	<section class="panel">
		<h2>Identity</h2>
		<label for="version-name">Program name as presented by this version</label>
		<input id="version-name" bind:value={content.name} {disabled} />
		<label for="version-label">Version label</label>
		<input
			id="version-label"
			bind:value={content.label}
			{disabled}
			placeholder="2026 CTO Program rev B"
		/>
		<label for="version-description">Description</label>
		<textarea id="version-description" bind:value={content.description} {disabled}></textarea>
	</section>

	<PhasesEditor
		bind:phases={content.phases}
		bind:transitions={content.phase_transitions}
		{disabled}
	/>
	<CompetenciesEditor bind:competencies={content.competencies} {disabled} />
	<ScalesEditor
		bind:scales={content.rating_scales}
		bind:modifiers={content.rating_modifiers}
		{disabled}
	/>
	<FormsEditor
		bind:forms={content.evaluation_forms}
		{competencyNames}
		{scaleNames}
		{disabled}
	/>
	<section class="panel">
		<h2>Program-level citations</h2>
		<CitationsEditor bind:citations={content.citations} {disabled} heading="" />
	</section>

	<div class="actionbar">
		<a class="back" href={`/programs/${programId}`}>Back to versions</a>
		<a href={versionExportPath(versionId)} download>Export</a>
		{#if canManage && !published}
			<button type="button" disabled={busy} onclick={save}>Save draft</button>
			<button type="button" disabled={busy} onclick={publish}>Publish</button>
			<button type="button" class="secondary" disabled={busy} onclick={discard}>
				Discard draft
			</button>
		{/if}
	</div>
{:else if error}
	<p class="error" role="alert">{error}</p>
{:else}
	<p>Loading…</p>
{/if}

<style>
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.75rem;
	}
	.quiet {
		opacity: 0.7;
		margin: 0 0 0.75rem;
	}
	form.enroll {
		margin-top: 1rem;
		max-width: 28rem;
	}
	.saved {
		background: light-dark(#e2f2e3, #1e3524);
		border: 1px solid light-dark(#a9d3ac, #2f5c38);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
		font-size: 0.92rem;
	}
	.actionbar {
		display: flex;
		gap: 0.75rem;
		align-items: center;
		position: sticky;
		bottom: 0;
		padding: 0.75rem 0;
		background: light-dark(#f4f5f7, #16181d);
		border-top: 1px solid light-dark(#d8dce3, #303642);
	}
	.actionbar .back {
		margin-right: auto;
	}
</style>
