<script lang="ts">
	import type { ModifierDef, ScaleDef, ScaleKind } from '$lib/api';

	let {
		scales = $bindable(),
		modifiers = $bindable(),
		disabled
	}: { scales: ScaleDef[]; modifiers: ModifierDef[]; disabled: boolean } = $props();

	const kinds: { value: ScaleKind; label: string }[] = [
		{ value: 'anchored_numeric', label: 'Anchored numeric' },
		{ value: 'pass_fail', label: 'Pass / fail' },
		{ value: 'narrative_only', label: 'Narrative only' }
	];

	function addScale() {
		scales.push({
			name: '',
			kind: 'anchored_numeric',
			min_value: 1,
			max_value: 7,
			anchors: [{ value: 1, label: '', definition: '' }]
		});
	}

	/** Keeps a scale's shape coherent with its kind (ADR 0007 kind rules). */
	function kindChanged(scale: ScaleDef) {
		if (scale.kind === 'anchored_numeric') {
			scale.min_value ??= 1;
			scale.max_value ??= 7;
			if (scale.anchors.length === 0) {
				scale.anchors.push({ value: scale.min_value, label: '', definition: '' });
			}
		} else if (scale.kind === 'pass_fail') {
			scale.min_value = null;
			scale.max_value = null;
			scale.anchors = [
				{ value: 0, label: 'Not demonstrated', definition: '' },
				{ value: 1, label: 'Demonstrated', definition: '' }
			];
		} else {
			scale.min_value = null;
			scale.max_value = null;
			scale.anchors = [];
		}
	}

	function addModifier() {
		modifiers.push({ code: '', label: '', description: '' });
	}
</script>

<section class="panel">
	<h2>Rating scales</h2>
	{#each scales as scale, index}
		<div class="item">
			<div class="row">
				<input
					aria-label="Scale name"
					placeholder="Scale name"
					bind:value={scale.name}
					{disabled}
				/>
				<select
					aria-label="Scale kind"
					bind:value={scale.kind}
					onchange={() => kindChanged(scale)}
					{disabled}
				>
					{#each kinds as kind (kind.value)}
						<option value={kind.value}>{kind.label}</option>
					{/each}
				</select>
				{#if !disabled}
					<button type="button" class="small secondary" onclick={() => scales.splice(index, 1)}>
						Remove
					</button>
				{/if}
			</div>
			{#if scale.kind === 'anchored_numeric'}
				<div class="row">
					<label class="inline" for={`scale-min-${index}`}>Range</label>
					<input
						id={`scale-min-${index}`}
						class="num"
						type="number"
						aria-label="Minimum value"
						bind:value={scale.min_value}
						{disabled}
					/>
					<span aria-hidden="true">to</span>
					<input
						class="num"
						type="number"
						aria-label="Maximum value"
						bind:value={scale.max_value}
						{disabled}
					/>
				</div>
			{/if}
			{#if scale.kind !== 'narrative_only'}
				<h4>Anchors</h4>
				{#each scale.anchors as anchor, anchorIndex}
					<div class="row">
						<input
							class="num"
							type="number"
							aria-label="Anchor value"
							bind:value={anchor.value}
							disabled={disabled || scale.kind === 'pass_fail'}
						/>
						<input
							aria-label="Anchor label"
							placeholder="Label"
							bind:value={anchor.label}
							{disabled}
						/>
						<input
							aria-label="Anchor definition"
							placeholder="Definition"
							bind:value={anchor.definition}
							{disabled}
						/>
						{#if !disabled && scale.kind === 'anchored_numeric'}
							<button
								type="button"
								class="small secondary"
								onclick={() => scale.anchors.splice(anchorIndex, 1)}
							>
								Remove
							</button>
						{/if}
					</div>
				{/each}
				{#if !disabled && scale.kind === 'anchored_numeric'}
					<button
						type="button"
						class="small secondary"
						onclick={() => scale.anchors.push({ value: 0, label: '', definition: '' })}
					>
						Add anchor
					</button>
				{/if}
			{/if}
		</div>
	{/each}
	{#if !disabled}
		<button type="button" class="secondary" onclick={addScale}>Add rating scale</button>
	{/if}

	<h3>Rating modifiers</h3>
	{#each modifiers as modifier, index}
		<div class="row">
			<input
				class="code"
				aria-label="Modifier code"
				placeholder="Code (e.g. NRT)"
				bind:value={modifier.code}
				{disabled}
			/>
			<input
				aria-label="Modifier label"
				placeholder="Label"
				bind:value={modifier.label}
				{disabled}
			/>
			<input
				aria-label="Modifier description"
				placeholder="Description"
				bind:value={modifier.description}
				{disabled}
			/>
			{#if !disabled}
				<button type="button" class="small secondary" onclick={() => modifiers.splice(index, 1)}>
					Remove
				</button>
			{/if}
		</div>
	{/each}
	{#if !disabled}
		<button type="button" class="small secondary" onclick={addModifier}>Add modifier</button>
	{/if}
</section>

<style>
	.item {
		border: 1px solid light-dark(#e3e6eb, #2a303b);
		border-radius: 6px;
		padding: 0.9rem;
		margin: 0 0 0.9rem;
	}
	h3 {
		font-size: 0.95rem;
		margin: 1rem 0 0.5rem;
	}
	h4 {
		font-size: 0.85rem;
		margin: 0.25rem 0 0.35rem;
		opacity: 0.8;
	}
	input.num {
		max-width: 5rem;
	}
	input.code {
		max-width: 9rem;
	}
	label.inline {
		margin: 0;
	}
</style>
