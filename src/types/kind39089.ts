import type { ParsedEvent } from 'src/types';

export interface Kind39089Parsed {
	list_identifier: string;
	people: string[];
	title?: string;
	description?: string;
	image?: string;
}

export function isKind39089(event: ParsedEvent<unknown>): event is ParsedEvent<Kind39089Parsed> {
	return event.kind === 39089;
}
