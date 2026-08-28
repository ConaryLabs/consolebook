<script lang="ts">
	import { goto } from '$app/navigation';
	import { ApiError, resetPassword } from '$lib/api';

	let username = $state('');
	let resetCode = $state('');
	let newPassword = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		busy = true;
		try {
			await resetPassword({
				username,
				reset_code: resetCode.trim(),
				new_password: newPassword
			});
			await goto('/login');
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Reset password</h1>
<p class="lede">
	Use the single-use code an administrator issued for you — or, for a locked-out
	administrator, the code from <code>consolebook recover</code> on the host.
	Resetting signs you out everywhere.
</p>

<form class="card" onsubmit={submit}>
	<label for="username">Username</label>
	<input id="username" autocomplete="username" required bind:value={username} />

	<label for="reset-code">Reset code</label>
	<input
		id="reset-code"
		autocomplete="one-time-code"
		required
		bind:value={resetCode}
	/>

	<label for="new-password">New password</label>
	<input
		id="new-password"
		type="password"
		autocomplete="new-password"
		required
		minlength="12"
		bind:value={newPassword}
	/>

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}
	<button type="submit" disabled={busy}>Set new password</button>
</form>

<p><a href="/login">Back to sign in</a></p>
