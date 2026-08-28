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
}

export class ApiError extends Error {
	readonly status: number;
	readonly code: string;

	constructor(status: number, body: ApiErrorBody) {
		super(body.message);
		this.status = status;
		this.code = body.error;
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
