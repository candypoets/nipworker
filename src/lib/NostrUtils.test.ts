import { describe, expect, it } from 'vitest';

import {
	extractTag,
	extractTagMap,
	extractTagValue,
	extractTagValues,
	readStringVec
} from './NostrUtils';

describe('NostrUtils tag helpers', () => {
	it('reads plain string vectors', () => {
		expect(readStringVec(['playerOut', 'alice'])).toEqual(['playerOut', 'alice']);
	});

	it('extracts values from a tag array', () => {
		const tags: Array<[string, ...string[]]> = [
			['playerOut', 'alice'],
			['playerIn', 'bob'],
			['assist', 'carol'],
			['p', 'pubkey-a', 'relay-a', 'reply'],
			['p', 'pubkey-b', 'relay-b', 'mention'],
			['title', 'Goal']
		];

		expect(extractTagValue(tags, 'playerOut')).toBe('alice');
		expect(extractTag(tags, 'assist')).toEqual(['assist', 'carol']);
		expect(extractTagValues(tags, 'playerOut')).toEqual(['alice']);
		expect(extractTagValues(tags, 'p', { where: (tag) => tag[3] === 'reply' })).toEqual([
			'pubkey-a'
		]);
		expect(extractTagMap(tags)).toEqual({
			playerOut: ['alice'],
			playerIn: ['bob'],
			assist: ['carol'],
			p: ['pubkey-a', 'relay-a', 'reply', 'pubkey-b', 'relay-b', 'mention'],
			title: ['Goal']
		});
		expect(extractTagMap(tags, { where: (tag) => tag[3] === 'reply' })).toEqual({
			p: ['pubkey-a', 'relay-a', 'reply']
		});
	});

	it('extracts values from a FlatBuffers-style tag collection', () => {
		const tagCollection = {
			tagsLength: () => 3,
			tags: (index: number): [string, ...string[]] | null =>
				[
					['playerOut', 'alice'],
					['playerIn', 'bob'],
					['assist', 'carol']
				][index] ?? null
		};

		expect(extractTagValue(tagCollection, 'playerIn')).toBe('bob');
		expect(extractTagValues(tagCollection, 'assist')).toEqual(['carol']);
	});
});
