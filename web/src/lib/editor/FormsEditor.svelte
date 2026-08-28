<script lang="ts">
	import type { FormDef, RecordType } from '$lib/api';

	let {
		forms = $bindable(),
		competencyNames,
		scaleNames,
		disabled
	}: {
		forms: FormDef[];
		competencyNames: string[];
		scaleNames: string[];
		disabled: boolean;
	} = $props();

	const recordTypes: { value: RecordType; label: string }[] = [
		{ value: 'daily_report', label: 'Daily report' },
		{ value: 'weekly_summary', label: 'Weekly summary' },
		{ value: 'phase_evaluation', label: 'Phase evaluation' }
	];

	function addForm() {
		forms.push({
			record_type: 'daily_report',
			name: '',
			instructions: '',
			competencies: [],
			narratives: []
		});
	}

	function addBinding(form: FormDef) {
		form.competencies.push({
			competency: competencyNames[0] ?? '',
			rating_scale: scaleNames[0] ?? ''
		});
	}
</script>

<section class="panel">
	<h2>Evaluation forms</h2>
	<p class="hint">
		The product owns the form skeleton; you configure which competencies it
		rates, on which scale, and its narrative prompts. A form needs at least
		one competency or narrative to publish.
	</p>
	{#each forms as form, index}
		<div class="item">
			<div class="row">
				<select aria-label="Record type" bind:value={form.record_type} {disabled}>
					{#each recordTypes as recordType (recordType.value)}
						<option value={recordType.value}>{recordType.label}</option>
					{/each}
				</select>
				<input aria-label="Form name" placeholder="Form name" bind:value={form.name} {disabled} />
				{#if !disabled}
					<button type="button" class="small secondary" onclick={() => forms.splice(index, 1)}>
						Remove
					</button>
				{/if}
			</div>
			<textarea
				aria-label="Form instructions"
				placeholder="Instructions shown on the form (optional)"
				bind:value={form.instructions}
				{disabled}
			></textarea>
			<h4>Rated competencies</h4>
			{#each form.competencies as binding, bindingIndex}
				<div class="row">
					<select aria-label="Competency" bind:value={binding.competency} {disabled}>
						{#each competencyNames as name (name)}
							<option value={name}>{name}</option>
						{/each}
					</select>
					<select aria-label="Rating scale" bind:value={binding.rating_scale} {disabled}>
						{#each scaleNames as name (name)}
							<option value={name}>{name}</option>
						{/each}
					</select>
					{#if !disabled}
						<button
							type="button"
							class="small secondary"
							onclick={() => form.competencies.splice(bindingIndex, 1)}
						>
							Remove
						</button>
					{/if}
				</div>
			{/each}
			{#if !disabled}
				<button
					type="button"
					class="small secondary"
					onclick={() => addBinding(form)}
					disabled={competencyNames.length === 0 || scaleNames.length === 0}
				>
					Add rated competency
				</button>
			{/if}
			<h4>Narratives</h4>
			{#each form.narratives as narrative, narrativeIndex}
				<div class="row">
					<input
						aria-label="Narrative prompt"
						placeholder="Narrative prompt"
						bind:value={narrative.prompt}
						{disabled}
					/>
					<label class="check">
						<input type="checkbox" bind:checked={narrative.required} {disabled} />
						required
					</label>
					{#if !disabled}
						<button
							type="button"
							class="small secondary"
							onclick={() => form.narratives.splice(narrativeIndex, 1)}
						>
							Remove
						</button>
					{/if}
				</div>
			{/each}
			{#if !disabled}
				<button
					type="button"
					class="small secondary"
					onclick={() => form.narratives.push({ prompt: '', required: true })}
				>
					Add narrative
				</button>
			{/if}
		</div>
	{/each}
	{#if !disabled}
		<button type="button" class="secondary" onclick={addForm}>Add evaluation form</button>
	{/if}
</section>

<style>
	.hint {
		font-size: 0.88rem;
		opacity: 0.75;
		margin: 0 0 0.75rem;
	}
	.item {
		border: 1px solid light-dark(#e3e6eb, #2a303b);
		border-radius: 6px;
		padding: 0.9rem;
		margin: 0 0 0.9rem;
	}
	h4 {
		font-size: 0.85rem;
		margin: 0.25rem 0 0.35rem;
		opacity: 0.8;
	}
	label.check {
		display: flex;
		align-items: center;
		gap: 0.35rem;
		font-weight: 400;
		white-space: nowrap;
		margin: 0;
	}
</style>
