<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { ShellData } from './+layout';

	let { data, children }: { data: ShellData; children: Snippet } = $props();
</script>

<div class="shell">
	<header>
		<span class="wordmark">Consolebook</span>
		{#if data.instance.agency}
			<span class="agency">{data.instance.agency}</span>
		{/if}
	</header>
	<main>
		{@render children()}
	</main>
	<footer>
		Consolebook {data.instance.version} · pre-alpha · AGPL-3.0-only
	</footer>
</div>

<style>
	:global(*) {
		box-sizing: border-box;
	}
	:global(body) {
		margin: 0;
		font-family:
			system-ui,
			-apple-system,
			'Segoe UI',
			Roboto,
			sans-serif;
		background: light-dark(#f4f5f7, #16181d);
		color: light-dark(#1d222b, #dfe3ea);
		line-height: 1.5;
	}
	.shell {
		min-height: 100vh;
		display: flex;
		flex-direction: column;
	}
	header {
		display: flex;
		align-items: baseline;
		gap: 0.75rem;
		padding: 0.9rem 1.5rem;
		background: light-dark(#1f2a3d, #0e1420);
		color: #f2f5fa;
	}
	.wordmark {
		font-weight: 700;
		letter-spacing: 0.02em;
	}
	.agency {
		opacity: 0.85;
		font-size: 0.95rem;
	}
	main {
		flex: 1;
		width: 100%;
		max-width: 44rem;
		margin: 0 auto;
		padding: 2rem 1.25rem 3rem;
	}
	footer {
		padding: 0.75rem 1.5rem;
		font-size: 0.8rem;
		opacity: 0.65;
		text-align: center;
	}

	:global(h1) {
		font-size: 1.45rem;
		margin: 0 0 0.25rem;
	}
	:global(p.lede) {
		margin: 0 0 1.5rem;
		opacity: 0.8;
	}
	:global(form.card, section.card) {
		background: light-dark(#ffffff, #1f232c);
		border: 1px solid light-dark(#d8dce3, #303642);
		border-radius: 8px;
		padding: 1.5rem;
		margin: 0 0 1.25rem;
	}
	:global(label) {
		display: block;
		font-weight: 600;
		font-size: 0.9rem;
		margin: 0 0 0.25rem;
	}
	:global(input) {
		width: 100%;
		font: inherit;
		padding: 0.5rem 0.65rem;
		margin: 0 0 1rem;
		border: 1px solid light-dark(#b9c0cb, #4a5261);
		border-radius: 6px;
		background: light-dark(#ffffff, #141821);
		color: inherit;
	}
	:global(input:focus-visible),
	:global(button:focus-visible),
	:global(a:focus-visible) {
		outline: 3px solid #4c8dff;
		outline-offset: 1px;
	}
	:global(button) {
		font: inherit;
		font-weight: 600;
		padding: 0.55rem 1.1rem;
		border: 0;
		border-radius: 6px;
		background: #2456a6;
		color: #ffffff;
		cursor: pointer;
	}
	:global(button.secondary) {
		background: transparent;
		color: inherit;
		border: 1px solid light-dark(#b9c0cb, #4a5261);
	}
	:global(button:disabled) {
		opacity: 0.6;
		cursor: default;
	}
	:global(p.error) {
		background: light-dark(#fbe9e9, #3a1d20);
		border: 1px solid light-dark(#e4b4b4, #7c3b41);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
		font-size: 0.92rem;
	}
	:global(a) {
		color: light-dark(#2456a6, #7fabf5);
	}
	:global(dl.facts) {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 0.35rem 1.25rem;
		margin: 0;
	}
	:global(dl.facts dt) {
		font-weight: 600;
		font-size: 0.9rem;
	}
	:global(dl.facts dd) {
		margin: 0;
	}
</style>
