<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { ApiError, completeSetup } from '$lib/api';

	let setupCode = $state('');
	let agencyName = $state('');
	let username = $state('');
	let displayName = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		busy = true;
		try {
			await completeSetup({
				setup_code: setupCode.trim(),
				agency_name: agencyName,
				username,
				display_name: displayName,
				password
			});
			await invalidateAll();
			await goto('/login');
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>First-run setup</h1>
<p class="lede">
	This installation is not initialized. The setup code is printed by the server
	when it starts, or by running <code>consolebook setup-code</code> on the host.
</p>

<form class="card" onsubmit={submit}>
	<label for="setup-code">Setup code</label>
	<input
		id="setup-code"
		autocomplete="one-time-code"
		required
		bind:value={setupCode}
	/>

	<label for="agency-name">Agency name</label>
	<input id="agency-name" required bind:value={agencyName} />

	<label for="username">Administrator username</label>
	<input id="username" autocomplete="username" required bind:value={username} />

	<label for="display-name">Administrator display name</label>
	<input id="display-name" autocomplete="name" bind:value={displayName} />

	<label for="password">Administrator password</label>
	<input
		id="password"
		type="password"
		autocomplete="new-password"
		required
		minlength="12"
		bind:value={password}
	/>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}
	<button type="submit" disabled={busy}>Initialize installation</button>
</form>
