// Typed client for the Consolebook HTTP API. Every call goes to the same
// origin; sessions ride the HttpOnly cookie, never JavaScript-visible state.

export interface Instance {
	initialized: boolean;
	version: string;
	agency: string | null;
}

export interface SessionUser {
	id: number;
	username: string;
	display_name: string;
}

export interface Session {
	user: SessionUser;
	capabilities: string[];
	expires_at: number;
}

export interface Health {
	status: string;
	version: string;
	database: string;
}

/** Error body shape the API guarantees for non-2xx responses. */
export interface ApiErrorBody {
	error: string;
	message: string;
	/** Itemized refusal reasons, present on validation refusals. */
	problems?: string[];
}

export class ApiError extends Error {
	readonly status: number;
	readonly code: string;
	readonly problems: string[];

	constructor(status: number, body: ApiErrorBody) {
		super(body.message);
		this.status = status;
		this.code = body.error;
		this.problems = body.problems ?? [];
	}
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
	const response = await fetch(path, {
		headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
		...init
	});
	if (response.status === 204) {
		return undefined as T;
	}
	if (!response.ok) {
		let body: ApiErrorBody;
		try {
			body = (await response.json()) as ApiErrorBody;
		} catch {
			body = { error: 'unreachable', message: `server returned ${response.status}` };
		}
		throw new ApiError(response.status, body);
	}
	return (await response.json()) as T;
}

export function getInstance(): Promise<Instance> {
	return request<Instance>('/api/instance');
}

export function getHealth(): Promise<Health> {
	return request<Health>('/api/health');
}

/** Resolves to null when no valid session cookie is present. */
export async function getSession(): Promise<Session | null> {
	try {
		return await request<Session>('/api/auth/session');
	} catch (error) {
		if (error instanceof ApiError && error.status === 401) {
			return null;
		}
		throw error;
	}
}

export function completeSetup(input: {
	setup_code: string;
	agency_name: string;
	username: string;
	display_name: string;
	password: string;
}): Promise<{ administrator_user_id: number }> {
	return request('/api/setup', { method: 'POST', body: JSON.stringify(input) });
}

export function login(username: string, password: string): Promise<Session> {
	return request('/api/auth/login', {
		method: 'POST',
		body: JSON.stringify({ username, password })
	});
}

export function logout(): Promise<void> {
	return request('/api/auth/logout', { method: 'POST', body: JSON.stringify({}) });
}

export function resetPassword(input: {
	username: string;
	reset_code: string;
	new_password: string;
}): Promise<void> {
	return request('/api/auth/reset', { method: 'POST', body: JSON.stringify(input) });
}

export interface Notice {
	id: number;
	kind: string;
	message: string;
	created_at: number;
	read_at: number | null;
}

export interface NoticesBody {
	notices: Notice[];
	unread: number;
}

export function getNotices(): Promise<NoticesBody> {
	return request('/api/notices');
}

export function markNoticeRead(id: number): Promise<void> {
	return request(`/api/notices/${id}/read`, { method: 'POST', body: JSON.stringify({}) });
}

export function issueResetCode(
	username: string
): Promise<{ username: string; reset_code: string; expires_at: number }> {
	return request('/api/auth/reset-codes', {
		method: 'POST',
		body: JSON.stringify({ username })
	});
}

// Program configuration (docs/formats/program-version-export.md documents
// the content document; field names mirror the server verbatim).

export type TransitionKind = 'advance' | 'remediation' | 'skip' | 'restart';
export type ScaleKind = 'anchored_numeric' | 'pass_fail' | 'narrative_only';
export type RecordType = 'daily_report' | 'weekly_summary' | 'phase_evaluation';

export interface CitationDef {
	body: string;
	edition: string;
	clause: string;
	note: string;
}

export interface PhaseDef {
	name: string;
	description: string;
	presentation_number: number;
}

export interface TransitionDef {
	from_phase: string;
	to_phase: string;
	kind: TransitionKind;
}

export interface TaskDef {
	prompt: string;
	citations: CitationDef[];
}

export interface CompetencyDef {
	category: string;
	name: string;
	description: string;
	tasks: TaskDef[];
	citations: CitationDef[];
}

export interface AnchorDef {
	value: number;
	label: string;
	definition: string;
}

export interface ScaleDef {
	name: string;
	kind: ScaleKind;
	min_value: number | null;
	max_value: number | null;
	anchors: AnchorDef[];
}

export interface ModifierDef {
	code: string;
	label: string;
	description: string;
}

export interface FormCompetencyDef {
	competency: string;
	rating_scale: string;
}

export interface NarrativeDef {
	prompt: string;
	required: boolean;
}

export interface FormDef {
	record_type: RecordType;
	name: string;
	instructions: string;
	competencies: FormCompetencyDef[];
	narratives: NarrativeDef[];
}

export interface PolicyDef {
	review_approved: boolean;
	required_narratives: boolean;
	ratings_complete: boolean;
}

export interface VersionContent {
	name: string;
	label: string;
	description: string;
	phases: PhaseDef[];
	phase_transitions: TransitionDef[];
	competencies: CompetencyDef[];
	rating_scales: ScaleDef[];
	rating_modifiers: ModifierDef[];
	evaluation_forms: FormDef[];
	citations: CitationDef[];
	finalization_policy: PolicyDef;
}

export interface ProgramSummary {
	id: number;
	name: string;
	created_at: number;
}

export interface VersionSummary {
	id: number;
	program_id: number;
	version_number: number;
	label: string;
	name: string;
	created_at: number;
	published_at: number | null;
}

export interface ProgramsBody {
	programs: ProgramSummary[];
}

export function listPrograms(): Promise<ProgramsBody> {
	return request('/api/programs');
}

export function createProgram(name: string): Promise<{ id: number }> {
	return request('/api/programs', { method: 'POST', body: JSON.stringify({ name }) });
}

export interface VersionsBody {
	program: ProgramSummary;
	versions: VersionSummary[];
}

export function getProgramVersions(programId: number): Promise<VersionsBody> {
	return request(`/api/programs/${programId}/versions`);
}

export function createVersion(
	programId: number,
	content: VersionContent
): Promise<{ id: number }> {
	return request(`/api/programs/${programId}/versions`, {
		method: 'POST',
		body: JSON.stringify(content)
	});
}

export interface VersionBody {
	summary: VersionSummary;
	content: VersionContent;
}

export function getVersion(versionId: number): Promise<VersionBody> {
	return request(`/api/program-versions/${versionId}`);
}

export function replaceVersionContent(
	versionId: number,
	content: VersionContent
): Promise<void> {
	return request(`/api/program-versions/${versionId}/content`, {
		method: 'PUT',
		body: JSON.stringify(content)
	});
}

export function publishVersion(versionId: number): Promise<void> {
	return request(`/api/program-versions/${versionId}/publish`, {
		method: 'POST',
		body: JSON.stringify({})
	});
}

export function discardVersion(versionId: number): Promise<void> {
	return request(`/api/program-versions/${versionId}`, { method: 'DELETE' });
}

/** Download URL for a version's export document (a browser navigation). */
export function versionExportPath(versionId: number): string {
	return `/api/program-versions/${versionId}/export`;
}

export interface ImportedBody {
	id: number;
	program_id: number;
}

export function importProgram(document: string): Promise<ImportedBody> {
	return request('/api/programs/import', {
		method: 'POST',
		body: JSON.stringify({ document })
	});
}

export function importNextVersion(
	programId: number,
	document: string
): Promise<ImportedBody> {
	return request(`/api/programs/${programId}/versions/import`, {
		method: 'POST',
		body: JSON.stringify({ document })
	});
}

// Users and enrollment (Milestone 3 slice 1: role bundles and profile
// fields at creation; full user administration is a later milestone).

export type Role = 'administrator' | 'coordinator' | 'trainer' | 'trainee';

export interface UserSummary {
	id: number;
	username: string;
	display_name: string;
	employee_id: string;
	title: string;
	created_at: number;
	capabilities: string[];
}

export function listUsers(): Promise<{ users: UserSummary[] }> {
	return request('/api/users');
}

export interface CreatedUser {
	id: number;
	username: string;
	display_name: string;
	reset_code: string;
	reset_expires_at: number;
}

export function createUser(input: {
	username: string;
	display_name: string;
	employee_id: string;
	title: string;
	role: Role;
}): Promise<CreatedUser> {
	return request('/api/users', { method: 'POST', body: JSON.stringify(input) });
}

export interface Enrollee {
	enrollment_id: number;
	user_id: number;
	username: string;
	display_name: string;
	enrolled_at: number;
	enrolled_by: number | null;
}

export function listEnrollments(versionId: number): Promise<{ enrollees: Enrollee[] }> {
	return request(`/api/program-versions/${versionId}/enrollments`);
}

export function enrollUser(versionId: number, userId: number): Promise<{ id: number }> {
	return request(`/api/program-versions/${versionId}/enrollments`, {
		method: 'POST',
		body: JSON.stringify({ user_id: userId })
	});
}

// Training lifecycle (Milestone 3 slice 1: assignments, enrollment
// lifecycle events, and phase history; field names mirror the server).

export type EnrollmentStatus = 'active' | 'withdrawn' | 'completed';
export type EnrollmentEventKind = 'version_change' | 'withdraw' | 'complete' | 'reinstate';
export type PhaseEventKind = 'advance' | 'return' | 'restart' | 'pause' | 'resume' | 'complete';

export interface EnrollmentEvent {
	id: number;
	kind: string;
	occurred_at: number;
	actor_user_id: number | null;
	actor_display_name: string | null;
	reason: string;
	from_program_version_id: number | null;
	from_version_number: number | null;
	from_version_label: string | null;
	to_program_version_id: number | null;
	to_version_number: number | null;
	to_version_label: string | null;
}

export interface PhaseEvent {
	id: number;
	kind: string;
	from_phase_id: number | null;
	from_phase_name: string | null;
	to_phase_id: number | null;
	to_phase_name: string | null;
	effective_at: number;
	recorded_at: number;
	actor_user_id: number | null;
	actor_display_name: string | null;
	reason: string;
}

export interface PhaseRef {
	id: number;
	name: string;
	presentation_number: number;
}

export interface TransitionRef {
	from_phase_id: number;
	to_phase_id: number;
	kind: TransitionKind;
}

export interface Assignment {
	id: number;
	enrollment_id: number;
	trainer_user_id: number;
	trainer_username: string;
	trainer_display_name: string;
	assigned_at: number;
	assigned_by: number | null;
	ended_at: number | null;
	ended_by: number | null;
}

export interface AssignedTrainee {
	assignment_id: number;
	enrollment_id: number;
	trainee_user_id: number;
	trainee_username: string;
	trainee_display_name: string;
	program_version_id: number;
	program_name: string;
	version_number: number;
	version_label: string;
	assigned_at: number;
}

export interface EnrollmentDetail {
	enrollment_id: number;
	trainee_user_id: number;
	trainee_username: string;
	trainee_display_name: string;
	enrolled_at: number;
	program_id: number;
	program_version_id: number;
	program_name: string;
	version_number: number;
	version_label: string;
	status: EnrollmentStatus;
	paused: boolean;
	current_phase_id: number | null;
	current_phase_name: string | null;
	events: EnrollmentEvent[];
	phase_events: PhaseEvent[];
	assignments: Assignment[];
	phases: PhaseRef[];
	transitions: TransitionRef[];
}

export function getEnrollment(enrollmentId: number): Promise<EnrollmentDetail> {
	return request(`/api/enrollments/${enrollmentId}`);
}

export function recordEnrollmentEvent(
	enrollmentId: number,
	input: { kind: EnrollmentEventKind; reason: string; to_version_id?: number }
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/events`, {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function recordPhaseEvent(
	enrollmentId: number,
	input: {
		kind: PhaseEventKind;
		to_phase_id?: number;
		effective_at?: number;
		reason: string;
	}
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/phase-events`, {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function createAssignment(
	enrollmentId: number,
	trainerUserId: number
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/assignments`, {
		method: 'POST',
		body: JSON.stringify({ trainer_user_id: trainerUserId })
	});
}

export function endAssignment(assignmentId: number): Promise<void> {
	return request(`/api/assignments/${assignmentId}/end`, {
		method: 'POST',
		body: JSON.stringify({})
	});
}

export function myAssignments(): Promise<{ assignments: AssignedTrainee[] }> {
	return request('/api/assignments/mine');
}

// Training sessions (Milestone 3 slice 2; ADR 0009): the entered local
// representation is stored verbatim, UTC is resolved server-side.

export type SessionDisposition = 'completed' | 'interrupted' | 'cancelled';

export interface SessionTrainer {
	user_id: number;
	username: string;
	display_name: string;
	added_at: number;
}

export interface TrainingSession {
	id: number;
	enrollment_id: number;
	business_date: string;
	timezone: string;
	local_start: string;
	local_end: string | null;
	utc_start: number;
	utc_end: number | null;
	phase_id: number | null;
	phase_name: string | null;
	disposition: SessionDisposition | null;
	created_at: number;
	created_by: number | null;
	closed_at: number | null;
	closed_by: number | null;
	draft_id: number | null;
	trainers: SessionTrainer[];
}

export interface MySession {
	session_id: number;
	enrollment_id: number;
	business_date: string;
	timezone: string;
	local_start: string;
	local_end: string | null;
	utc_start: number;
	disposition: SessionDisposition | null;
	phase_name: string | null;
	trainee_user_id: number;
	trainee_username: string;
	trainee_display_name: string;
	program_name: string;
	version_number: number;
	draft_id: number | null;
}

export function listSessions(
	enrollmentId: number
): Promise<{ sessions: TrainingSession[] }> {
	return request(`/api/enrollments/${enrollmentId}/sessions`);
}

export function createSession(
	enrollmentId: number,
	input: {
		business_date: string;
		timezone: string;
		local_start: string;
		local_end?: string;
		disposition?: SessionDisposition;
		phase_id?: number;
		trainer_user_ids: number[];
	}
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/sessions`, {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function updateSession(
	sessionId: number,
	input: {
		business_date: string;
		timezone: string;
		local_start: string;
		phase_id?: number;
	}
): Promise<void> {
	return request(`/api/sessions/${sessionId}`, {
		method: 'PUT',
		body: JSON.stringify(input)
	});
}

export function closeSession(
	sessionId: number,
	disposition: SessionDisposition,
	localEnd?: string
): Promise<void> {
	return request(`/api/sessions/${sessionId}/close`, {
		method: 'POST',
		body: JSON.stringify({ disposition, local_end: localEnd })
	});
}

export function addSessionTrainer(
	sessionId: number,
	trainerUserId: number
): Promise<void> {
	return request(`/api/sessions/${sessionId}/trainers`, {
		method: 'POST',
		body: JSON.stringify({ trainer_user_id: trainerUserId })
	});
}

export function removeSessionTrainer(sessionId: number, userId: number): Promise<void> {
	return request(`/api/sessions/${sessionId}/trainers/${userId}`, { method: 'DELETE' });
}

export function mySessions(): Promise<{ sessions: MySession[] }> {
	return request('/api/sessions/mine');
}

export type DraftStatus =
	| 'draft'
	| 'submitted'
	| 'changes_requested'
	| 'returned'
	| 'approved'
	| 'finalized';

export type ReviewDecisionKind = 'approved' | 'changes_requested' | 'returned';

export interface ReviewDecision {
	id: number;
	reviewer_user_id: number;
	reviewer_display_name: string;
	decision: ReviewDecisionKind;
	comment: string;
	decided_at: number;
}

export interface ReviewQueueRow {
	record_id: number;
	trainee_user_id: number;
	trainee_display_name: string;
	owner_display_name: string;
	program_name: string;
	version_number: number;
	submitted_at: number;
	eligible: boolean;
}

export interface ContributorEvent {
	id: number;
	kind: string;
	actor_user_id: number;
	actor_display_name: string;
	to_user_id: number | null;
	to_display_name: string | null;
	recorded_at: number;
}

export interface CoveredSession {
	session_id: number;
	business_date: string;
	timezone: string;
	local_start: string;
	local_end: string | null;
}

export interface SnapshotMeta {
	id: number;
	reason: string;
	taken_at: number;
	taken_by: number | null;
}

export interface EligibleRecipient {
	user_id: number;
	display_name: string;
}

export interface SkeletonAnchor {
	value: number;
	label: string;
	definition: string;
}

export interface SkeletonCompetency {
	form_competency_id: number;
	category: string;
	name: string;
	description: string;
	scale_name: string;
	scale_kind: 'anchored_numeric' | 'pass_fail' | 'narrative_only';
	min_value: number | null;
	max_value: number | null;
	anchors: SkeletonAnchor[];
}

export interface SkeletonNarrative {
	form_narrative_id: number;
	prompt: string;
	required: boolean;
}

export interface SkeletonModifier {
	rating_modifier_id: number;
	code: string;
	label: string;
	description: string;
}

export interface RatingEntry {
	form_competency_id: number;
	value: number | null;
	not_observed: boolean;
	modifier_ids: number[];
}

export interface NarrativeEntry {
	form_narrative_id: number;
	text: string;
}

export interface DraftContent {
	ratings: RatingEntry[];
	narratives: NarrativeEntry[];
}

export interface DraftView {
	id: number;
	enrollment_id: number;
	program_version_id: number;
	evaluation_form_id: number;
	owner_user_id: number;
	owner_display_name: string;
	status: DraftStatus;
	trainee_user_id: number;
	trainee_display_name: string;
	program_name: string;
	version_number: number;
	sessions: CoveredSession[];
	events: ContributorEvent[];
	snapshots: SnapshotMeta[];
	eligible_recipients: EligibleRecipient[];
	decisions: ReviewDecision[];
	record_type: RecordType;
	summary_links: SummaryLink[];
	viewer_may_review: boolean;
	viewer_may_finalize: boolean;
	viewer_may_amend: boolean;
	latest_version_number: number | null;
	open_amendment: AmendmentView | null;
	created_at: number;
	revision: number;
	form: {
		form_name: string;
		instructions: string;
		competencies: SkeletonCompetency[];
		narratives: SkeletonNarrative[];
		modifiers: SkeletonModifier[];
	};
	content: DraftContent;
}

export function createDraft(sessionId: number, formId?: number): Promise<{ id: number }> {
	return request(`/api/sessions/${sessionId}/draft`, {
		method: 'POST',
		body: JSON.stringify({ evaluation_form_id: formId })
	});
}

export function dailyForms(
	sessionId: number
): Promise<{ forms: { id: number; name: string }[] }> {
	return request(`/api/sessions/${sessionId}/daily-forms`);
}

export function getDraft(draftId: number): Promise<DraftView> {
	return request(`/api/drafts/${draftId}`);
}

export function saveDraftContent(
	draftId: number,
	revision: number,
	content: DraftContent
): Promise<{ revision: number }> {
	return request(`/api/drafts/${draftId}/content`, {
		method: 'PUT',
		body: JSON.stringify({ revision, ...content })
	});
}

export function transferDraft(draftId: number, toUserId: number): Promise<void> {
	return request(`/api/drafts/${draftId}/transfer`, {
		method: 'POST',
		body: JSON.stringify({ to_user_id: toUserId })
	});
}

export function submitDraft(draftId: number, revision: number): Promise<void> {
	return request(`/api/drafts/${draftId}/submit`, {
		method: 'POST',
		body: JSON.stringify({ revision })
	});
}

export function reviewDraft(
	draftId: number,
	decision: ReviewDecisionKind,
	comment?: string
): Promise<void> {
	return request(`/api/drafts/${draftId}/review`, {
		method: 'POST',
		body: JSON.stringify({ decision, comment })
	});
}

export function reviewQueue(): Promise<{ drafts: ReviewQueueRow[] }> {
	return request('/api/reviews/queue');
}

export interface VersionMeta {
	version_number: number;
	record_schema: number;
	content_hash: string;
	chain_hash: string;
	finalized_at: number;
	finalized_by: number;
	finalized_by_display_name: string;
}

/** One identity as the finalized envelope presents it. */
export interface EnvelopeUser {
	id: number;
	username: string;
	display_name: string;
}

/** The stored canonical envelope (record schema 1, ADR 0011). */
export interface RecordEnvelope {
	attachments: unknown[];
	attribution: {
		kind: string;
		actor: EnvelopeUser;
		to: EnvelopeUser | null;
		recorded_at: number;
	}[];
	canonicalization: string;
	content: {
		narratives: { prompt: string; required: boolean; text: string | null }[];
		ratings: {
			competency: { category: string; name: string; description: string; tasks: string[] };
			scale: {
				name: string;
				kind: string;
				min_value: number | null;
				max_value: number | null;
				anchors: { value: number; label: string; definition: string }[];
			};
			value: number | null;
			not_observed: boolean;
			modifiers: { code: string; label: string; description: string }[];
		}[];
	};
	finalization: {
		finalized_at: number;
		finalized_by: EnvelopeUser;
		policy: PolicyDef;
	};
	form: { name: string; instructions: string; record_type: string };
	instance: string;
	program: { name: string; version_number: number; label: string };
	/** Schema 2 (ADR 0013): the exact daily versions a summary covers. */
	daily_reports?: { content_hash: string; record_id: number; version_number: number }[];
	record: {
		id: number;
		version_number: number;
		record_schema: number;
		predecessor_content_hash: string | null;
	};
	review: { reviewer: EnvelopeUser; decision: string; comment: string; decided_at: number }[];
	sessions: {
		business_date: string;
		timezone: string;
		local_start: string;
		local_end: string | null;
		utc_start: number;
		utc_end: number | null;
		disposition: string | null;
		phase: { name: string; presentation_number: number } | null;
		trainers: EnvelopeUser[];
	}[];
	trainee: EnvelopeUser & { employee_id: string; title: string };
}

export interface FinalizedView {
	meta: VersionMeta;
	envelope: RecordEnvelope;
}

export interface Verification {
	content_hash_ok: boolean;
	chain_hash_ok: boolean;
}

export function finalizeDraft(draftId: number, revision: number): Promise<VersionMeta> {
	return request(`/api/drafts/${draftId}/finalize`, {
		method: 'POST',
		body: JSON.stringify({ revision })
	});
}

export function finalizedVersion(draftId: number): Promise<FinalizedView> {
	return request(`/api/drafts/${draftId}/version`);
}

export function verifyVersion(draftId: number): Promise<Verification> {
	return request(`/api/drafts/${draftId}/version/verify`);
}

// Acknowledgments and the trainee's own-records timeline (Milestone 4
// slice 2). Acknowledgment means receipt, not agreement.

export type TraineeAckKind = 'acknowledged' | 'acknowledged_with_response' | 'refused';
export type AttestedKind = 'supervisor_attested_refusal' | 'unavailable';
export type AckKind = TraineeAckKind | AttestedKind;

export interface Acknowledgment {
	kind: AckKind;
	response: string;
	user_display_name: string;
	recorded_by: number;
	recorded_by_display_name: string;
	recorded_at: number;
}

export interface TimelineRecord {
	record_id: number;
	program_name: string;
	version_number: number;
	form_name: string;
	business_date: string | null;
	finalized_at: number;
	/** The latest version's number; above 1 the record was amended. */
	record_version_number: number;
	acknowledgment_kind: AckKind | null;
	acknowledged_at: number | null;
}

// Amendments (Milestone 4 slice 3): a correction produces a successor
// version linked to the original with a reason and authority; the
// original stays readable while retained.

export interface AmendmentView {
	reason: string;
	opened_by_display_name: string;
	opened_at: number;
}

export interface VersionHistoryRow {
	version_number: number;
	record_schema: number;
	content_hash: string;
	chain_hash: string;
	finalized_at: number;
	finalized_by_display_name: string;
	amendment: AmendmentView | null;
	acknowledgment: Acknowledgment | null;
}

export function amendRecord(draftId: number, reason: string): Promise<void> {
	return request(`/api/drafts/${draftId}/amend`, {
		method: 'POST',
		body: JSON.stringify({ reason })
	});
}

export function versionHistory(
	draftId: number
): Promise<{ versions: VersionHistoryRow[] }> {
	return request(`/api/drafts/${draftId}/versions`);
}

/** One retained version by number — a superseded original included. */
export function finalizedVersionAt(
	draftId: number,
	versionNumber: number
): Promise<FinalizedView> {
	return request(`/api/drafts/${draftId}/versions/${versionNumber}`);
}

export function verifyVersionAt(
	draftId: number,
	versionNumber: number
): Promise<Verification> {
	return request(`/api/drafts/${draftId}/versions/${versionNumber}/verify`);
}

export function acknowledgeRecord(
	draftId: number,
	kind: TraineeAckKind,
	response?: string
): Promise<void> {
	return request(`/api/drafts/${draftId}/acknowledge`, {
		method: 'POST',
		body: JSON.stringify({ kind, response })
	});
}

export function attestRecord(
	draftId: number,
	kind: AttestedKind,
	reason: string
): Promise<void> {
	return request(`/api/drafts/${draftId}/attest`, {
		method: 'POST',
		body: JSON.stringify({ kind, reason })
	});
}

export function getAcknowledgment(
	draftId: number
): Promise<{ acknowledgment: Acknowledgment | null }> {
	return request(`/api/drafts/${draftId}/acknowledgment`);
}

export function myRecords(): Promise<{ records: TimelineRecord[] }> {
	return request('/api/my/records');
}

// Weekly summaries and task signoffs (Milestone 4 slice 4; ADR 0013).

export interface SummaryLink {
	daily_version_id: number;
	record_id: number;
	version_number: number;
	content_hash: string;
	finalized_at: number;
	form_name: string | null;
	business_date: string | null;
}

export function summaryForms(
	enrollmentId: number
): Promise<{ forms: { id: number; name: string }[] }> {
	return request(`/api/enrollments/${enrollmentId}/summary-forms`);
}

export function createWeeklySummary(
	enrollmentId: number,
	formId?: number
): Promise<{ id: number }> {
	return request(`/api/enrollments/${enrollmentId}/weekly-summary`, {
		method: 'POST',
		body: JSON.stringify({ evaluation_form_id: formId })
	});
}

export function linkableDailies(draftId: number): Promise<{ dailies: SummaryLink[] }> {
	return request(`/api/drafts/${draftId}/linkable-dailies`);
}

export function addSummaryLink(
	draftId: number,
	dailyVersionId: number,
	revision: number
): Promise<{ revision: number }> {
	return request(`/api/drafts/${draftId}/links`, {
		method: 'POST',
		body: JSON.stringify({ daily_version_id: dailyVersionId, revision })
	});
}

export function removeSummaryLink(
	draftId: number,
	dailyVersionId: number,
	revision: number
): Promise<{ revision: number }> {
	return request(`/api/drafts/${draftId}/links/remove`, {
		method: 'POST',
		body: JSON.stringify({ daily_version_id: dailyVersionId, revision })
	});
}

export type SignoffKind = 'observed' | 'demonstrated' | 'revoked';

export interface SignoffTask {
	task_id: number;
	competency_category: string;
	competency_name: string;
	prompt: string;
	kind: SignoffKind | null;
	reason: string | null;
	signed_by_display_name: string | null;
	signed_at: number | null;
	history: number;
}

export function signoffMatrix(
	enrollmentId: number
): Promise<{ tasks: SignoffTask[] }> {
	return request(`/api/enrollments/${enrollmentId}/signoffs`);
}

export function recordSignoff(
	enrollmentId: number,
	taskId: number,
	kind: SignoffKind,
	reason?: string
): Promise<void> {
	return request(`/api/enrollments/${enrollmentId}/signoffs`, {
		method: 'POST',
		body: JSON.stringify({ task_id: taskId, kind, reason })
	});
}

/** A structurally valid empty draft for starting a program from scratch. */
export function blankContent(name: string): VersionContent {
	return {
		name,
		label: '',
		description: '',
		phases: [],
		phase_transitions: [],
		competencies: [],
		rating_scales: [],
		rating_modifiers: [],
		evaluation_forms: [],
		citations: [],
		finalization_policy: {
			review_approved: true,
			required_narratives: true,
			ratings_complete: true
		}
	};
}

// Record exports (Milestone 5 slice 1; docs/formats/record-export.md;
// ADR 0014): the stored canonical bytes travel verbatim beside
// manifests, and an archive verifies from its own contents alone.

export interface ExportSummary {
	installation_id: string;
	record_count: number;
	version_count: number;
}

export function exportSummary(): Promise<ExportSummary> {
	return request('/api/exports/summary');
}

/** Download path for one finalized version. */
export function recordVersionExportPath(recordId: number, versionNumber: number): string {
	return `/api/drafts/${recordId}/versions/${versionNumber}/export`;
}

/** Download path for every retained version of a record. */
export function recordExportPath(recordId: number): string {
	return `/api/drafts/${recordId}/export`;
}

/** Download path for every finalized version of an enrollment's records. */
export function enrollmentExportPath(enrollmentId: number): string {
	return `/api/enrollments/${enrollmentId}/export`;
}

/** Download path for every finalized version the installation holds. */
export function installationExportPath(): string {
	return '/api/exports/records';
}

/**
 * Fetches an export archive and hands it to the browser as a download, so
 * a refusal surfaces as an error instead of a saved error document.
 * Returns the server's file name.
 */
export async function downloadExport(path: string): Promise<string> {
	const response = await fetch(path);
	if (!response.ok) {
		let body: ApiErrorBody;
		try {
			body = (await response.json()) as ApiErrorBody;
		} catch {
			body = { error: 'unreachable', message: `server returned ${response.status}` };
		}
		throw new ApiError(response.status, body);
	}
	const disposition = response.headers.get('Content-Disposition') ?? '';
	const named = /filename="([^"]+)"/.exec(disposition);
	const fileName = named?.[1] ?? 'consolebook-export.zip';
	const url = URL.createObjectURL(await response.blob());
	const anchor = document.createElement('a');
	anchor.href = url;
	anchor.download = fileName;
	document.body.append(anchor);
	anchor.click();
	anchor.remove();
	// Revoke once the browser has had time to start the download.
	setTimeout(() => URL.revokeObjectURL(url), 60_000);
	return fileName;
}

// Trainee packets (Milestone 5 slice 2; docs/formats/trainee-packet.md;
// ADR 0015): everything retained about one enrollment as one archive.

export interface OwnEnrollment {
	enrollment_id: number;
	program_name: string;
	version_number: number;
	version_label: string;
	enrolled_at: number;
	status: EnrollmentStatus;
	finalized_versions: number;
}

export function myEnrollments(): Promise<{ enrollments: OwnEnrollment[] }> {
	return request('/api/my/enrollments');
}

/** Download path for an enrollment's trainee packet. */
export function enrollmentPacketPath(enrollmentId: number): string {
	return `/api/enrollments/${enrollmentId}/packet`;
}
