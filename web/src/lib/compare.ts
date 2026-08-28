// Structural comparison of two program-version content documents,
// rendered as human-readable section diffs. Items are matched by their
// identity within the document (names, codes, prompts, endpoints) — the
// same keys the format spec makes unique.

import type { CitationDef, CompetencyDef, FormDef, ScaleDef, VersionContent } from '$lib/api';

export interface SectionDiff {
	title: string;
	added: string[];
	removed: string[];
	changed: string[];
	unchanged: number;
}

interface KeyedDiff {
	added: string[];
	removed: string[];
	changed: string[];
	unchanged: number;
}

function diffByKey<T>(
	before: T[],
	after: T[],
	key: (item: T) => string,
	changes: (before: T, after: T) => string[]
): KeyedDiff {
	const result: KeyedDiff = { added: [], removed: [], changed: [], unchanged: 0 };
	const beforeByKey = new Map(before.map((item) => [key(item), item]));
	const afterByKey = new Map(after.map((item) => [key(item), item]));
	for (const [k, item] of beforeByKey) {
		if (!afterByKey.has(k)) {
			result.removed.push(key(item));
		}
	}
	for (const [k, item] of afterByKey) {
		const previous = beforeByKey.get(k);
		if (previous === undefined) {
			result.added.push(key(item));
			continue;
		}
		const changed = changes(previous, item);
		if (changed.length > 0) {
			result.changed.push(`${k}: ${changed.join(', ')}`);
		} else {
			result.unchanged += 1;
		}
	}
	return result;
}

function fieldChanges<T>(before: T, after: T, fields: [keyof T, string][]): string[] {
	const changed: string[] = [];
	for (const [field, label] of fields) {
		if (before[field] !== after[field]) {
			changed.push(`${label} changed`);
		}
	}
	return changed;
}

function citationKey(citation: CitationDef): string {
	const edition = citation.edition === '' ? '' : ` ${citation.edition}`;
	const note = citation.note === '' ? '' : ` (${citation.note})`;
	return `${citation.body}${edition} — ${citation.clause}${note}`;
}

function citationChanges(before: CitationDef[], after: CitationDef[]): string[] {
	const diff = diffByKey(before, after, citationKey, () => []);
	return [
		...diff.added.map((c) => `citation added: ${c}`),
		...diff.removed.map((c) => `citation removed: ${c}`)
	];
}

function competencyChanges(before: CompetencyDef, after: CompetencyDef): string[] {
	const changed = fieldChanges(before, after, [
		['category', 'category'],
		['description', 'description']
	]);
	const tasks = diffByKey(before.tasks, after.tasks, (task) => task.prompt, (a, b) =>
		citationChanges(a.citations, b.citations)
	);
	changed.push(
		...tasks.added.map((prompt) => `task added: '${prompt}'`),
		...tasks.removed.map((prompt) => `task removed: '${prompt}'`),
		...tasks.changed.map((description) => `task ${description}`),
		...citationChanges(before.citations, after.citations)
	);
	return changed;
}

function scaleChanges(before: ScaleDef, after: ScaleDef): string[] {
	const changed = fieldChanges(before, after, [
		['kind', 'kind'],
		['min_value', 'minimum'],
		['max_value', 'maximum']
	]);
	const anchors = diffByKey(
		before.anchors,
		after.anchors,
		(anchor) => String(anchor.value),
		(a, b) =>
			fieldChanges(a, b, [
				['label', 'label'],
				['definition', 'definition']
			])
	);
	changed.push(
		...anchors.added.map((value) => `anchor ${value} added`),
		...anchors.removed.map((value) => `anchor ${value} removed`),
		...anchors.changed.map((description) => `anchor ${description}`)
	);
	return changed;
}

function formChanges(before: FormDef, after: FormDef): string[] {
	const changed = fieldChanges(before, after, [
		['record_type', 'record type'],
		['instructions', 'instructions']
	]);
	const bindings = diffByKey(
		before.competencies,
		after.competencies,
		(binding) => binding.competency,
		(a, b) => (a.rating_scale === b.rating_scale ? [] : ['rating scale changed'])
	);
	changed.push(
		...bindings.added.map((name) => `now rates '${name}'`),
		...bindings.removed.map((name) => `no longer rates '${name}'`),
		...bindings.changed
	);
	const narratives = diffByKey(
		before.narratives,
		after.narratives,
		(narrative) => narrative.prompt,
		(a, b) => (a.required === b.required ? [] : ['required changed'])
	);
	changed.push(
		...narratives.added.map((prompt) => `narrative added: '${prompt}'`),
		...narratives.removed.map((prompt) => `narrative removed: '${prompt}'`),
		...narratives.changed.map((description) => `narrative ${description}`)
	);
	return changed;
}

function section(title: string, diff: KeyedDiff): SectionDiff {
	return { title, ...diff };
}

/** Compares two content documents section by section. */
export function compareContent(before: VersionContent, after: VersionContent): SectionDiff[] {
	const header: SectionDiff = {
		title: 'Identity',
		added: [],
		removed: [],
		changed: fieldChanges(before, after, [
			['name', 'program name'],
			['label', 'label'],
			['description', 'description']
		]),
		unchanged: 0
	};
	return [
		header,
		section(
			'Phases',
			diffByKey(before.phases, after.phases, (phase) => phase.name, (a, b) =>
				fieldChanges(a, b, [
					['description', 'description'],
					['presentation_number', 'number']
				])
			)
		),
		section(
			'Transitions',
			diffByKey(
				before.phase_transitions,
				after.phase_transitions,
				(transition) => `${transition.from_phase} → ${transition.to_phase}`,
				(a, b) => (a.kind === b.kind ? [] : ['kind changed'])
			)
		),
		section(
			'Competencies',
			diffByKey(before.competencies, after.competencies, (c) => c.name, competencyChanges)
		),
		section(
			'Rating scales',
			diffByKey(before.rating_scales, after.rating_scales, (s) => s.name, scaleChanges)
		),
		section(
			'Rating modifiers',
			diffByKey(before.rating_modifiers, after.rating_modifiers, (m) => m.code, (a, b) =>
				fieldChanges(a, b, [
					['label', 'label'],
					['description', 'description']
				])
			)
		),
		section(
			'Evaluation forms',
			diffByKey(before.evaluation_forms, after.evaluation_forms, (f) => f.name, formChanges)
		),
		section('Program citations', {
			added: [],
			removed: [],
			changed: citationChanges(before.citations, after.citations),
			unchanged: 0
		})
	];
}
