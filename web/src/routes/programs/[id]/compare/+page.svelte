<script lang="ts">
	import { page } from '$app/state';
	import { ApiError, getVersion, type VersionBody } from '$lib/api';
	import { compareContent, type SectionDiff } from '$lib/compare';

	let programId = $derived(Number(page.params.id));
	let fromId = $derived(Number(page.url.searchParams.get('a')));
	let toId = $derived(Number(page.url.searchParams.get('b')));

	let from: VersionBody | null = $state(null);
	let to: VersionBody | null = $state(null);
	let error = $state('');

	$effect(() => {
		error = '';
		Promise.all([getVersion(fromId), getVersion(toId)]).then(
			([a, b]) => {
				from = a;
				to = b;
			},
			(err) => {
				error = err instanceof ApiError ? err.message : 'the server could not be reached';
			}
		);
	});

	let sections: SectionDiff[] = $derived.by(() => {
		const a = from;
		const b = to;
		return a !== null && b !== null ? compareContent(a.content, b.content) : [];
	});
	let anyDifference = $derived(
		sections.some((s) => s.added.length + s.removed.length + s.changed.length > 0)
	);
</script>

<h1>Compare versions</h1>
{#if from !== null && to !== null}
	<p class="lede">
		v{from.summary.version_number}
		{from.summary.label} → v{to.summary.version_number}
		{to.summary.label}
	</p>
{/if}

{#if error}
	<p class="error" role="alert">{error}</p>
{:else if from !== null && to !== null}
	{#if !anyDifference}
		<section class="panel">
			<p class="quiet">These versions have identical content.</p>
		</section>
	{/if}
	{#each sections as section (section.title)}
		{#if section.added.length + section.removed.length + section.changed.length > 0}
			<section class="panel">
				<h2>{section.title}</h2>
				<ul class="diff">
					{#each section.added as entry}
						<li class="added">Added: {entry}</li>
					{/each}
					{#each section.removed as entry}
						<li class="removed">Removed: {entry}</li>
					{/each}
					{#each section.changed as entry}
						<li class="changed">{entry}</li>
					{/each}
				</ul>
				{#if section.unchanged > 0}
					<p class="quiet">{section.unchanged} unchanged</p>
				{/if}
			</section>
		{/if}
	{/each}
{:else}
	<p>Loading…</p>
{/if}

<p><a href={`/programs/${programId}`}>Back to versions</a></p>

<style>
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.75rem;
	}
	.quiet {
		opacity: 0.7;
		margin: 0.5rem 0 0;
	}
	ul.diff {
		margin: 0;
		padding-left: 1.4rem;
	}
	ul.diff li {
		margin: 0 0 0.25rem;
	}
	li.added {
		color: light-dark(#1e5c28, #9fd3a8);
	}
	li.removed {
		color: light-dark(#8f2f2f, #e0a1a1);
	}
</style>
