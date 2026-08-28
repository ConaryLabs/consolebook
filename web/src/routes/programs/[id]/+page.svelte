<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		ApiError,
		blankContent,
		createVersion,
		discardVersion,
		getProgramVersions,
		getVersion,
		importNextVersion,
		publishVersion,
		versionExportPath,
		type ProgramSummary,
		type VersionSummary
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from '../../+layout';

	let { data }: { data: ShellData } = $props();
	let canManage = $derived(data.session?.capabilities.includes('manage_programs') ?? false);
	let programId = $derived(Number(page.params.id));

	let program: ProgramSummary | null = $state(null);
	let versions: VersionSummary[] = $state([]);
	let error = $state('');
	let busy = $state(false);

	async function reload() {
		try {
			const body = await getProgramVersions(programId);
			program = body.program;
			versions = body.versions;
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		}
	}

	$effect(() => {
		void reload();
	});

	async function act(action: () => Promise<void>) {
		error = '';
		busy = true;
		try {
			await action();
			await reload();
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	async function newBlankDraft() {
		await act(async () => {
			const created = await createVersion(programId, blankContent(program?.name ?? ''));
			await goto(`/programs/${programId}/versions/${created.id}`);
		});
	}

	async function draftFrom(version: VersionSummary) {
		await act(async () => {
			const source = await getVersion(version.id);
			const created = await createVersion(programId, source.content);
			await goto(`/programs/${programId}/versions/${created.id}`);
		});
	}

	async function publish(version: VersionSummary) {
		if (!window.confirm(`Publish version ${version.version_number}? Published versions are permanently immutable.`)) {
			return;
		}
		await act(() => publishVersion(version.id));
	}

	async function discard(version: VersionSummary) {
		if (!window.confirm(`Discard draft version ${version.version_number}? Its content is deleted.`)) {
			return;
		}
		await act(() => discardVersion(version.id));
	}

	let importFiles: FileList | null = $state(null);

	async function importDocument(event: SubmitEvent) {
		event.preventDefault();
		const file = importFiles?.item(0);
		if (!file) {
			error = 'choose an export file first';
			return;
		}
		const document = await file.text();
		await act(async () => {
			const imported = await importNextVersion(programId, document);
			await goto(`/programs/${programId}/versions/${imported.id}`);
		});
	}

	let compareA = $state(0);
	let compareB = $state(0);
	let comparable = $derived(versions.length >= 2);

	async function compare(event: SubmitEvent) {
		event.preventDefault();
		await goto(`/programs/${programId}/compare?a=${compareA}&b=${compareB}`);
	}
</script>

<h1>{program?.name ?? 'Program'}</h1>
<p class="lede">Versions are immutable once published; corrections are new versions.</p>

{#if error}
	<p class="error" role="alert">{error}</p>
{/if}

<section class="panel">
	<h2>Versions</h2>
	{#if versions.length === 0}
		<p class="quiet">No versions yet.</p>
	{:else}
		<table class="grid">
			<thead>
				<tr>
					<th>Version</th>
					<th>Label</th>
					<th>Status</th>
					<th>Created</th>
					<th>Actions</th>
				</tr>
			</thead>
			<tbody>
				{#each versions as version (version.id)}
					<tr>
						<td>
							<a href={`/programs/${programId}/versions/${version.id}`}>
								v{version.version_number}
							</a>
						</td>
						<td>{version.label}</td>
						<td>
							{#if version.published_at === null}
								<span class="pill draft">Draft</span>
							{:else}
								<span class="pill published">Published {instant(version.published_at)}</span>
							{/if}
						</td>
						<td>{instant(version.created_at)}</td>
						<td class="actions">
							<a href={versionExportPath(version.id)} download>Export</a>
							{#if canManage}
								{#if version.published_at === null}
									<button
										type="button"
										class="small"
										disabled={busy}
										onclick={() => publish(version)}
									>
										Publish
									</button>
									<button
										type="button"
										class="small secondary"
										disabled={busy}
										onclick={() => discard(version)}
									>
										Discard
									</button>
								{:else}
									<button
										type="button"
										class="small secondary"
										disabled={busy}
										onclick={() => draftFrom(version)}
									>
										New draft from this
									</button>
								{/if}
							{/if}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
	{#if canManage}
		<div class="row toolbar">
			<button type="button" class="secondary" disabled={busy} onclick={newBlankDraft}>
				New blank draft
			</button>
		</div>
	{/if}
</section>

{#if comparable}
	<form class="card" onsubmit={compare}>
		<h2>Compare versions</h2>
		<div class="row">
			<select aria-label="Compare from" bind:value={compareA}>
				<option value={0} disabled>From…</option>
				{#each versions as version (version.id)}
					<option value={version.id}>v{version.version_number} {version.label}</option>
				{/each}
			</select>
			<select aria-label="Compare to" bind:value={compareB}>
				<option value={0} disabled>To…</option>
				{#each versions as version (version.id)}
					<option value={version.id}>v{version.version_number} {version.label}</option>
				{/each}
			</select>
			<button type="submit" class="secondary" disabled={compareA === 0 || compareB === 0}>
				Compare
			</button>
		</div>
	</form>
{/if}

{#if canManage}
	<form class="card" onsubmit={importDocument}>
		<h2>Import as next version</h2>
		<label for="version-import">Export file</label>
		<input
			id="version-import"
			type="file"
			accept="application/json,.json"
			bind:files={importFiles}
		/>
		<button type="submit" class="secondary" disabled={busy}>Import draft</button>
	</form>
{/if}

<style>
	.quiet {
		opacity: 0.7;
		margin: 0;
	}
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.75rem;
	}
	td.actions {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.toolbar {
		margin-top: 1rem;
	}
</style>
