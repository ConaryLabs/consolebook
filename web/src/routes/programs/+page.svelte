<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		ApiError,
		createProgram,
		importProgram,
		listPrograms,
		type ProgramSummary
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from '../+layout';

	let { data }: { data: ShellData } = $props();
	let canManage = $derived(data.session?.capabilities.includes('manage_programs') ?? false);

	let programs: ProgramSummary[] = $state([]);
	let loaded = $state(false);
	$effect(() => {
		listPrograms().then(
			(body) => {
				programs = body.programs;
				loaded = true;
			},
			() => {
				programs = [];
				loaded = true;
			}
		);
	});

	let newName = $state('');
	let error = $state('');
	let busy = $state(false);

	async function create(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		busy = true;
		try {
			const created = await createProgram(newName);
			await goto(`/programs/${created.id}`);
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}

	let importFiles: FileList | null = $state(null);
	let importError = $state('');

	async function importDocument(event: SubmitEvent) {
		event.preventDefault();
		importError = '';
		const file = importFiles?.item(0);
		if (!file) {
			importError = 'choose an export file first';
			return;
		}
		busy = true;
		try {
			const imported = await importProgram(await file.text());
			await goto(`/programs/${imported.program_id}`);
		} catch (err) {
			importError = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Training programs</h1>
<p class="lede">Versioned program configuration for this agency.</p>

<section class="panel">
	{#if programs.length === 0}
		<p class="quiet">
			{loaded ? 'No programs yet.' : 'Loading…'}
		</p>
	{:else}
		<table class="grid">
			<thead>
				<tr>
					<th>Program</th>
					<th>Created</th>
				</tr>
			</thead>
			<tbody>
				{#each programs as program (program.id)}
					<tr>
						<td><a href={`/programs/${program.id}`}>{program.name}</a></td>
						<td>{instant(program.created_at)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</section>

{#if canManage}
	<form class="card" onsubmit={create}>
		<h2>New program</h2>
		<label for="program-name">Program name</label>
		<input id="program-name" required bind:value={newName} placeholder="Communications Training Officer Program" />
		{#if error}
			<p class="error" role="alert">{error}</p>
		{/if}
		<button type="submit" disabled={busy}>Create program</button>
	</form>

	<form class="card" onsubmit={importDocument}>
		<h2>Import a program</h2>
		<label for="program-import">Export file</label>
		<input id="program-import" type="file" accept="application/json,.json" bind:files={importFiles} />
		{#if importError}
			<p class="error" role="alert">{importError}</p>
		{/if}
		<button type="submit" class="secondary" disabled={busy}>Import as new program</button>
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
</style>
