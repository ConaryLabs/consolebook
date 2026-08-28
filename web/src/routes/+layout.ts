// Single-page app: no server-side rendering, no prerendered pages beyond
// the SPA fallback. The Rust executable serves the built assets.
export const ssr = false;
export const prerender = false;

import { redirect } from '@sveltejs/kit';
import type { LayoutLoad } from './$types';
import { getInstance, getSession, type Instance, type Session } from '$lib/api';

export interface ShellData {
	instance: Instance;
	session: Session | null;
}

/**
 * Routing guard: land on setup until initialized, on sign-in until
 * authenticated, and keep authenticated users out of the entry pages.
 */
export const load: LayoutLoad = async ({ url }): Promise<ShellData> => {
	const instance = await getInstance();
	const session = instance.initialized ? await getSession() : null;
	const path = url.pathname;

	if (!instance.initialized && path !== '/setup') {
		redirect(307, '/setup');
	}
	if (instance.initialized && path === '/setup') {
		redirect(307, '/');
	}
	if (instance.initialized && session === null && path !== '/login' && path !== '/reset') {
		redirect(307, '/login');
	}
	if (session !== null && (path === '/login' || path === '/reset')) {
		redirect(307, '/');
	}
	return { instance, session };
};
