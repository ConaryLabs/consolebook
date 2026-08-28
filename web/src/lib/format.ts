// Presentation helpers shared across pages.

/** Renders a UTC unix-seconds instant in the viewer's locale. */
export function instant(unixSeconds: number): string {
	return new Date(unixSeconds * 1000).toLocaleString();
}
