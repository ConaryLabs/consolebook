<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { ApiError, getHealth, issueResetCode, logout, type Health } from '$lib/api';
	import type { ShellData } from './+layout';

	let { data }: { data: ShellData } = $props();

	// The layout guard redirects unauthenticated visitors, so session is
	// present here; the fallback satisfies the type system honestly.
	let session = $derived(data.session);
	let canManageUsers = $derived(
		data.session?.capabilities.includes('manage_users') ?? false
	);

	let health: Health | null = $state(null);
	$effect(() => {
		getHealth().then(
			(h) => (health = h),
			() => (health = null)
		);
	});

	let resetUsername = $state('');
	let issued: { username: string; reset_code: string; expires_at: number } | null =
		$state(null);
	let issueError = $state('');
	let busy = $state(false);

	function instant(unixSeconds: number): string {
		return new Date(unixSeconds * 1000).toLocaleString();
	}

	async function signOut() {
		await logout();
		await invalidateAll();
		await goto('/login');
	}

	async function issueCode(event: SubmitEvent) {
		event.preventDefault();
		issueError = '';
		issued = null;
		busy = true;
		try {
			issued = await issueResetCode(resetUsername.trim());
		} catch (err) {
			issueError =
				err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			busy = false;
		}
	}
</script>

<h1>Installation status</h1>
<p class="lede">Signed in as {session?.user.display_name}.</p>

<section class="card">
	<dl class="facts">
		<dt>Agency</dt>
		<dd>{data.instance.agency}</dd>
		<dt>Signed in as</dt>
		<dd>{session?.user.username}</dd>
		<dt>Capabilities</dt>
		<dd>{session?.capabilities.join(', ') || 'none'}</dd>
		<dt>Session expires</dt>
		<dd>{session ? instant(session.expires_at) : '—'}</dd>
		<dt>Server version</dt>
		<dd>{data.instance.version}</dd>
		<dt>Database</dt>
		<dd>{health ? health.database : 'checking…'}</dd>
	</dl>
</section>

{#if canManageUsers}
	<section class="card">
		<h2>Issue a password reset code</h2>
		<p>
			The code is single-use, valid for 15 minutes, and shown only here — relay
			it to the user directly. Using it signs them out everywhere.
		</p>
		<form onsubmit={issueCode}>
			<label for="reset-username">Username</label>
			<input id="reset-username" required bind:value={resetUsername} />
			{#if issueError}
				<p class="error" role="alert">{issueError}</p>
			{/if}
			{#if issued}
				<p class="code-out">
					Reset code for <strong>{issued.username}</strong>:
					<code>{issued.reset_code}</code>
					(valid until {instant(issued.expires_at)})
				</p>
			{/if}
			<button type="submit" disabled={busy}>Issue reset code</button>
		</form>
	</section>
{/if}

<button class="secondary" onclick={signOut}>Sign out</button>

<style>
	h2 {
		font-size: 1.1rem;
		margin: 0 0 0.5rem;
	}
	.code-out {
		background: light-dark(#eef4ee, #1e2a1f);
		border: 1px solid light-dark(#bcd4bc, #3c553d);
		border-radius: 6px;
		padding: 0.6rem 0.8rem;
		font-size: 0.92rem;
		overflow-wrap: anywhere;
	}
	code {
		font-size: 1.05em;
	}
</style>
