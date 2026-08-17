import { describe, expect, it, vi } from 'vitest';

vi.mock('node:fs', () => ({
	readFileSync: vi.fn(() => new Uint8Array([0, 97, 115, 109]))
}));

import { nipworkerWasmPlugin } from './vite';

describe('nipworkerWasmPlugin', () => {
	it('emits WASM next to the copied nipworker workers', () => {
		const plugin = nipworkerWasmPlugin();
		const emitFile = vi.fn();
		const error = vi.fn((message: string) => {
			throw new Error(message);
		});
		const generateBundle = plugin.generateBundle;

		if (typeof generateBundle !== 'function') {
			throw new Error('generateBundle hook is not callable');
		}

		generateBundle.call(
			{ emitFile, error } as never,
			{} as never,
			{
				'_app/immutable/assets/cache.js': {
					type: 'asset',
					fileName: '_app/immutable/assets/cache.js',
					name: 'cache.js',
					source:
						'new URL("../wasm/nipworker_cache_bg.wasm", import.meta.url);' +
						'new URL("../wasm/nipworker_parser_bg.wasm", import.meta.url);'
				}
			} as never,
			false
		);

		expect(emitFile).toHaveBeenCalledTimes(2);
		expect(emitFile).toHaveBeenCalledWith(
			expect.objectContaining({
				type: 'asset',
				fileName: '_app/immutable/wasm/nipworker_cache_bg.wasm'
			})
		);
		expect(emitFile).toHaveBeenCalledWith(
			expect.objectContaining({
				type: 'asset',
				fileName: '_app/immutable/wasm/nipworker_parser_bg.wasm'
			})
		);
	});
});
