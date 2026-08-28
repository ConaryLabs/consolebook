<script lang="ts">
	import type { CompetencyDef } from '$lib/api';
	import CitationsEditor from './CitationsEditor.svelte';

	let {
		competencies = $bindable(),
		disabled
	}: { competencies: CompetencyDef[]; disabled: boolean } = $props();

	function addCompetency() {
		competencies.push({ category: '', name: '', description: '', tasks: [], citations: [] });
	}

	function addTask(competency: CompetencyDef) {
		competency.tasks.push({ prompt: '', citations: [] });
	}
</script>

<section class="panel">
	<h2>Competencies and tasks</h2>
	{#each competencies as competency, index}
		<div class="item">
			<div class="row">
				<input
					class="category"
					aria-label="Category"
					placeholder="Category (optional)"
					bind:value={competency.category}
					{disabled}
				/>
				<input
					aria-label="Competency name"
					placeholder="Competency name"
					bind:value={competency.name}
					{disabled}
				/>
				{#if !disabled}
					<button
						type="button"
						class="small secondary"
						onclick={() => competencies.splice(index, 1)}
					>
						Remove
					</button>
				{/if}
			</div>
			<div class="row">
				<input
					aria-label="Competency description"
					placeholder="Description"
					bind:value={competency.description}
					{disabled}
				/>
			</div>
			<CitationsEditor
				bind:citations={competency.citations}
				{disabled}
				heading="Competency citations"
			/>
			<div class="tasks">
				<h4>Tasks</h4>
				{#each competency.tasks as task, taskIndex}
					<div class="subitem">
						<div class="row">
							<input
								aria-label="Task prompt"
								placeholder="Task prompt"
								bind:value={task.prompt}
								{disabled}
							/>
							{#if !disabled}
								<button
									type="button"
									class="small secondary"
									onclick={() => competency.tasks.splice(taskIndex, 1)}
								>
									Remove
								</button>
							{/if}
						</div>
						<CitationsEditor bind:citations={task.citations} {disabled} heading="Task citations" />
					</div>
				{/each}
				{#if !disabled}
					<button type="button" class="small secondary" onclick={() => addTask(competency)}>
						Add task
					</button>
				{/if}
			</div>
		</div>
	{/each}
	{#if !disabled}
		<button type="button" class="secondary" onclick={addCompetency}>Add competency</button>
	{/if}
</section>

<style>
	.item {
		border: 1px solid light-dark(#e3e6eb, #2a303b);
		border-radius: 6px;
		padding: 0.9rem;
		margin: 0 0 0.9rem;
	}
	input.category {
		max-width: 14rem;
	}
	.tasks h4 {
		font-size: 0.85rem;
		margin: 0 0 0.35rem;
		opacity: 0.8;
	}
	.subitem {
		border-left: 3px solid light-dark(#e3e6eb, #2a303b);
		padding-left: 0.75rem;
		margin: 0 0 0.5rem;
	}
</style>
