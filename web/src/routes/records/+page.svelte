<script lang="ts">
	import {
		ApiError,
		downloadExport,
		enrollmentPacketPath,
		myEnrollments,
		myRecords,
		type AckKind,
		type OwnEnrollment,
		type TimelineRecord
	} from '$lib/api';
	import { instant } from '$lib/format';
	import type { ShellData } from '../+layout';

	let { data }: { data: ShellData } = $props();
	let canViewOwn = $derived(
		data.session?.capabilities.includes('view_own_records') ?? false
	);

	let records: TimelineRecord[] = $state([]);
	let loaded = $state(false);
	let error = $state('');

	$effect(() => {
		if (!canViewOwn) {
			loaded = true;
			return;
		}
		myRecords().then(
			(body) => {
				records = body.records;
				loaded = true;
			},
			(err: unknown) => {
				error =
					err instanceof ApiError ? err.message : 'the server could not be reached';
				loaded = true;
			}
		);
	});

	// A packet is everything retained about one enrollment, as one archive
	// the trainee can verify anywhere (ADR 0015).
	let enrollments: OwnEnrollment[] = $state([]);
	let packetError = $state('');
	let packetDownloaded = $state('');
	let packetBusy = $state(false);
	$effect(() => {
		if (!canViewOwn) {
			return;
		}
		myEnrollments().then(
			(body) => (enrollments = body.enrollments),
			() => (enrollments = [])
		);
	});
	async function downloadPacket(enrollment: OwnEnrollment) {
		packetError = '';
		packetDownloaded = '';
		packetBusy = true;
		try {
			packetDownloaded = await downloadExport(enrollmentPacketPath(enrollment.enrollment_id));
		} catch (err) {
			packetError = err instanceof ApiError ? err.message : 'the server could not be reached';
		} finally {
			packetBusy = false;
		}
	}
	function statusLabel(status: OwnEnrollment['status']): string {
		switch (status) {
			case 'active':
				return 'Active';
			case 'withdrawn':
				return 'Withdrawn';
			case 'completed':
				return 'Completed';
		}
	}

	function ackLabel(kind: AckKind | null): string {
		switch (kind) {
			case 'acknowledged':
				return 'Acknowledged';
			case 'acknowledged_with_response':
				return 'Acknowledged with response';
			case 'refused':
				return 'Refused';
			case 'supervisor_attested_refusal':
				return 'Refusal attested';
			case 'unavailable':
				return 'Recorded unavailable';
			case null:
				return 'Awaiting acknowledgment';
		}
	}
</script>

<svelte:head>
	<title>My records — Consolebook</title>
</svelte:head>

<h1>My records</h1>
<p class="lede">
	Your finalized training records. Acknowledgment records receipt, not
	agreement.
</p>

{#if canViewOwn}
	<section class="panel">
		<h2>My packets</h2>
		<p class="quiet">
			A packet is everything retained about one enrollment — every finalized
			version of every record, acknowledgments, amendments, signoff history,
			and the enrollment's own history — as one archive. Verify it anywhere
			with <code>consolebook-server export verify</code>.
		</p>
		{#if enrollments.length === 0}
			<p class="quiet">No enrollments.</p>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Program</th>
						<th>Status</th>
						<th>Finalized versions</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each enrollments as enrollment (enrollment.enrollment_id)}
						<tr>
							<td>
								{enrollment.program_name} — v{enrollment.version_number}
								{#if enrollment.version_label}
									({enrollment.version_label})
								{/if}
							</td>
							<td>{statusLabel(enrollment.status)}</td>
							<td>{enrollment.finalized_versions}</td>
							<td>
								<button
									type="button"
									class="secondary"
									disabled={packetBusy}
									onclick={() => downloadPacket(enrollment)}
								>
									Download packet
								</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if packetDownloaded}
			<p class="quiet" role="status">Downloaded {packetDownloaded}.</p>
		{/if}
		{#if packetError}
			<p class="error" role="alert">{packetError}</p>
		{/if}
	</section>
{/if}

{#if !canViewOwn}
	<p class="error" role="alert">This page is not available to this account.</p>
{:else if !loaded}
	<p class="quiet">Loading…</p>
{:else if error}
	<p class="error" role="alert">{error}</p>
{:else if records.length === 0}
	<section class="card">
		<p class="quiet">
			No finalized records yet. A record appears here once it is finalized.
		</p>
	</section>
{:else}
	<section class="panel">
		<table class="grid">
			<thead>
				<tr>
					<th>Date</th>
					<th>Form</th>
					<th>Program</th>
					<th>Finalized</th>
					<th>Acknowledgment</th>
					<th></th>
				</tr>
			</thead>
			<tbody>
				{#each records as record (record.record_id)}
					<tr>
						<td>{record.business_date ?? '—'}</td>
						<td>{record.form_name}</td>
						<td>{record.program_name} — v{record.version_number}</td>
						<td>{instant(record.finalized_at)}</td>
						<td>
							<span class="pill" class:pending={record.acknowledgment_kind === null}>
								{ackLabel(record.acknowledgment_kind)}
							</span>
							{#if record.record_version_number > 1}
								<span class="pill">Amended — v{record.record_version_number}</span>
							{/if}
						</td>
						<td><a href={`/drafts/${record.record_id}`}>Open</a></td>
					</tr>
				{/each}
			</tbody>
		</table>
	</section>
{/if}

<style>
	.quiet {
		opacity: 0.7;
		margin: 0;
	}
	.pill.pending {
		background: light-dark(#fdf1d7, #3b301e);
		color: light-dark(#7a5410, #e4c26d);
	}
</style>
