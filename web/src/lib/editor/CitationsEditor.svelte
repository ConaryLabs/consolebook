<script lang="ts">
	import type { CitationDef } from '$lib/api';

	let {
		citations = $bindable(),
		disabled,
		heading = 'Standards citations'
	}: { citations: CitationDef[]; disabled: boolean; heading?: string } = $props();

	function add() {
		citations.push({ body: '', edition: '', clause: '', note: '' });
	}
</script>

<div class="citations">
	{#if heading !== '' && (citations.length > 0 || !disabled)}
		<h4>{heading}</h4>
	{/if}
	{#each citations as citation, index}
		<div class="row">
			<input
				aria-label="Standards body"
				placeholder="Standards body"
				bind:value={citation.body}
				{disabled}
			/>
			<input
				class="narrow"
				aria-label="Edition"
				placeholder="Edition"
				bind:value={citation.edition}
				{disabled}
			/>
			<input
				class="narrow"
				aria-label="Clause"
				placeholder="Clause"
				bind:value={citation.clause}
				{disabled}
			/>
			<input aria-label="Note" placeholder="Note" bind:value={citation.note} {disabled} />
			{#if !disabled}
				<button
					type="button"
					class="small secondary"
					onclick={() => citations.splice(index, 1)}
				>
					Remove
				</button>
			{/if}
		</div>
	{/each}
	{#if !disabled}
		<button type="button" class="small secondary" onclick={add}>Add citation</button>
	{/if}
</div>

<style>
	.citations {
		margin: 0.25rem 0 1rem;
	}
	h4 {
		font-size: 0.85rem;
		margin: 0 0 0.35rem;
		opacity: 0.8;
	}
	input.narrow {
		max-width: 8rem;
	}
</style>
