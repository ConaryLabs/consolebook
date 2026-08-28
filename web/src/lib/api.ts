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
		citations: []
	};
}
