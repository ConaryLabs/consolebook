<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { ApiError, login } from '$lib/api';

	let username = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		busy = true;
		try {
			await login(username, password);
			await invalidateAll();
			await goto('/');
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Sign in</h1>
<p class="lede">Training records for your communications center.</p>

<form class="card" onsubmit={submit}>
	<label for="username">Username</label>
	<input id="username" autocomplete="username" required bind:value={username} />

	<label for="password">Password</label>
	<input
		id="password"
		type="password"
		autocomplete="current-password"
		required
		bind:value={password}
	/>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}
	<button type="submit" disabled={busy}>Sign in</button>
</form>

<p>
	Locked out with a reset code? <a href="/reset">Use a password reset code</a>.
</p>
