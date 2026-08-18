import { beforeEach, describe, expect, it, vi } from 'vitest';

const { generateSecretKey, getPublicKey, setNip46QR } = vi.hoisted(() => ({
	generateSecretKey: vi.fn(),
	getPublicKey: vi.fn(),
	setNip46QR: vi.fn()
}));

vi.mock('nostr-tools', () => ({ generateSecretKey, getPublicKey }));
vi.mock('./manager', () => ({
	getManager: () => ({ setNip46QR })
}));

import { connectWithQRCode, NIP46_REQUIRED_PERMISSIONS } from './utils';

describe('connectWithQRCode', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		generateSecretKey.mockReturnValue(Uint8Array.from({ length: 32 }, (_, index) => index));
		getPublicKey.mockReturnValueOnce('a'.repeat(64)).mockReturnValueOnce('b'.repeat(64));
	});

	it('advertises every required signer permission', async () => {
		const url = await connectWithQRCode('Nuts', ['wss://relay.one', 'wss://relay.two/path?test=1']);
		const parsed = new URL(url);

		expect(parsed.protocol).toBe('nostrconnect:');
		expect(parsed.hostname).toBe('a'.repeat(64));
		expect(parsed.searchParams.getAll('relay')).toEqual([
			'wss://relay.one',
			'wss://relay.two/path?test=1'
		]);
		expect(parsed.searchParams.get('name')).toBe('Nuts');
		expect(parsed.searchParams.get('secret')).toBe('b'.repeat(64));
		expect(parsed.searchParams.get('perms')).toBe(NIP46_REQUIRED_PERMISSIONS.join(','));
		expect(setNip46QR).toHaveBeenCalledWith(
			url,
			Array.from({ length: 32 }, (_, index) => index.toString(16).padStart(2, '0')).join('')
		);
	});
});
