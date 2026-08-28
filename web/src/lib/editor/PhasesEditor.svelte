<script lang="ts">
	import type { PhaseDef, TransitionDef, TransitionKind } from '$lib/api';

	let {
		phases = $bindable(),
		transitions = $bindable(),
		disabled
	}: { phases: PhaseDef[]; transitions: TransitionDef[]; disabled: boolean } = $props();

	const kinds: TransitionKind[] = ['advance', 'remediation', 'skip', 'restart'];
	let phaseNames = $derived(phases.map((p) => p.name));

	function addPhase() {
		phases.push({ name: '', description: '', presentation_number: phases.length + 1 });
	}

	function addTransition() {
		transitions.push({
			from_phase: phaseNames[0] ?? '',
			to_phase: phaseNames[0] ?? '',
			kind: 'advance'
		});
	}
</script>

<section class="panel">
	<h2>Phases</h2>
	<p class="hint">
		Optional — annual and in-service programs have no phases. The number is
		presentation order, never progress.
	</p>
	{#each phases as phase, index}
		<div class="row">
			<input
				class="num"
				type="number"
				aria-label="Phase number"
				bind:value={phase.presentation_number}
				{disabled}
			/>
			<input aria-label="Phase name" placeholder="Phase name" bind:value={phase.name} {disabled} />
			<input
				aria-label="Phase description"
				placeholder="Description"
				bind:value={phase.description}
				{disabled}
			/>
			{#if !disabled}
				<button type="button" class="small secondary" onclick={() => phases.splice(index, 1)}>
					Remove
				</button>
			{/if}
		</div>
	{/each}
	{#if !disabled}
		<button type="button" class="small secondary" onclick={addPhase}>Add phase</button>
	{/if}

	{#if phases.length > 0}
		<h3>Allowed transitions</h3>
		{#each transitions as transition, index}
			<div class="row">
				<select aria-label="From phase" bind:value={transition.from_phase} {disabled}>
					{#each phaseNames as name (name)}
						<option value={name}>{name}</option>
					{/each}
				</select>
				<span aria-hidden="true">→</span>
				<select aria-label="To phase" bind:value={transition.to_phase} {disabled}>
					{#each phaseNames as name (name)}
						<option value={name}>{name}</option>
					{/each}
				</select>
				<select aria-label="Transition kind" bind:value={transition.kind} {disabled}>
					{#each kinds as kind (kind)}
						<option value={kind}>{kind}</option>
					{/each}
				</select>
				{#if !disabled}
					<button
						type="button"
						class="small secondary"
						onclick={() => transitions.splice(index, 1)}
					>
						Remove
					</button>
				{/if}
			</div>
		{/each}
		{#if !disabled}
			<button type="button" class="small secondary" onclick={addTransition}>
				Add transition
			</button>
		{/if}
	{/if}
</section>

<style>
	.hint {
		font-size: 0.88rem;
		opacity: 0.75;
		margin: 0 0 0.75rem;
	}
	h3 {
		font-size: 0.95rem;
		margin: 1rem 0 0.5rem;
	}
	input.num {
		max-width: 5rem;
	}
</style>
