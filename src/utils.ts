export { ConnectionTracker } from './lib/ConnectionTracker';
export { ByteString } from './lib/ByteString';

export * from './lib/NarrowTypes';
export * from './lib/ParseContent';

/**
 * Extracts keys from T where the value is a `function(i: number): any`
 */
type FlatBufferKeys<T> = {
	[K in keyof T]: T[K] extends (i: number) => any ? K : never;
}[keyof T];

/**
 * Non-nullable version: Filters out null results
 */
export function fbIterable<T, K extends FlatBufferKeys<T>>(
	obj: T,
	fieldName: K
): Iterable<NonNullable<ReturnType<Extract<T[K], (i: number) => any>>>> {
	const length = (obj as any)[`${String(fieldName)}Length`]();
	return {
		[Symbol.iterator](): Iterator<NonNullable<ReturnType<Extract<T[K], (i: number) => any>>>> {
			let i = 0;
			return {
				next(): IteratorResult<NonNullable<ReturnType<Extract<T[K], (i: number) => any>>>> {
					while (i < length) {
						const value = (obj as any)[fieldName](i);
						i++;
						if (value != null) {
							return { value, done: false };
						}
						// if null → skip this index
					}
					return { value: undefined as any, done: true };
				}
			};
		}
	};
}

/**
 * Eager array version (non-null)
 */
export function fbArray<T, K extends FlatBufferKeys<T>>(
	obj: T,
	fieldName: K
): Array<NonNullable<ReturnType<Extract<T[K], (i: number) => any>>>> {
	if (!obj) return [];
	const lengthGetter = (obj as any)[`${String(fieldName)}Length`]?.bind(obj);
	if (!lengthGetter) return [];
	const length = lengthGetter();

	const result: Array<NonNullable<ReturnType<Extract<T[K], (i: number) => any>>>> = [];
	for (let i = 0; i < length; i++) {
		const v = (obj as any)[fieldName](i);
		if (v != null) result.push(v);
	}
	return result;
}
