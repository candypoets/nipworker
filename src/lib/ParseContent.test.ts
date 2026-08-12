import { describe, expect, it } from 'vitest';
import * as flatbuffers from 'flatbuffers';

import { ContentBlock } from 'src/generated/nostr/fb/content-block';
import { ContentData } from 'src/generated/nostr/fb/content-data';
import { LightningDataT } from 'src/generated/nostr/fb/lightning-data';

import { asLightningData } from './NarrowTypes';
import { parseContent } from './ParseContent';

const decoder = new TextDecoder();
const decode = (value: string | Uint8Array | null): string =>
	typeof value === 'string' ? value : decoder.decode(value ?? undefined);
const validBolt11 = [
	'lnbc1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5',
	'qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdpl2pkx2ctnv5sxx',
	'mmwwd5kgetjypeh2ursdae8g6twvus8g6rfwvs8qun0dfjkxaq9qrsgq357wnc5r2ueh7c',
	'k6q93dj32dlqnls087fxdwk8qakdyafkq3yap9us6v52vjjsrvywa6rt52cm9r9zqt8r2',
	't7mlcwspyetp5h2tztugp9lfyql'
].join('');
const validDescriptionHashBolt11 = [
	'lnbc20m1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqq',
	'syqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqhp58yjmdan79s6qqdhdzgynm4zw',
	'qd5d7xmw5fk98klysy043l2ahrqs9qrsgq7ea976txfraylvgzuxs8kgcw23ezlrszfnh8r6qtfp',
	'r6cxga50aj6txm9rxrydzd06dfeawfk6swupvz4erwnyutnjq7x39ymw6j38gp7ynn44'
].join('');
const invalidDescriptionHashLength = [
	'lnbc20m1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqq',
	'syqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqhpn8yjmdan79s6qqdhdzgynm4zw',
	'qd5d7xmw5fk98klysy043l2ahrqs9qrsgq7ea976txfraylvgzuxs8kgcw23ezlrszfnh8r6qtfp',
	'r6cxga50aj6txm9rxrydzd06dfeawfk6swupvz4erwnyutnjq7x39ymw6j38gphrv8jd'
].join('');
const invalidImpreciseAmount = [
	'lnbc2500000001p1pvjluezpp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqy',
	'pqdq5xysxxatsyp3k7enxv4jsxqzpusp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg',
	'3zyg3zygs9qrsgq0lzc236j96a95uv0m3umg28gclm5lqxtqqwk32uuk4k6673k6n5kfvx3d2h8s',
	'295fad45fdhmusm8sjudfhlf6dcsxmfvkeywmjdkxcp99202x'
].join('');
const invalidZeroRScalar = [
	'lnbc1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyq',
	'cyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdpl2pkx2ctnv5sxxmmwwd5kgetjype',
	'h2ursdae8g6twvus8g6rfwvs8qun0dfjkxaq9qrsgqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq',
	'qqqqqqqqqqqqqqqqq9us6v52vjjsrvywa6rt52cm9r9zqt8r2t7mlcwspyetp5h2tztugpc4vhg3'
].join('');

describe('parseContent Lightning invoices', () => {
	it('parses a bare BOLT11 invoice', async () => {
		const blocks = await parseContent(`Pay this invoice: ${validBolt11}`);
		const block = blocks.find((candidate) => candidate.dataType === ContentData.LightningData);

		expect(block).toBeDefined();
		expect(decode(block?.type ?? null)).toBe('lightning');
		expect(decode(block?.text ?? null)).toBe(validBolt11);
		expect(decode((block?.data as LightningDataT).invoice)).toBe(validBolt11);
	});

	it('parses a lightning URI and exposes the invoice without the scheme', async () => {
		const invoice = validBolt11.toUpperCase();
		const uri = `LIGHTNING:${invoice}`;
		const [block] = await parseContent(uri);

		expect(block?.dataType).toBe(ContentData.LightningData);
		expect(decode(block?.text ?? null)).toBe(uri);
		expect(decode((block?.data as LightningDataT).invoice)).toBe(invoice);
	});

	it('parses an invoice with a description hash', async () => {
		const [block] = await parseContent(validDescriptionHashBolt11);

		expect(block?.dataType).toBe(ContentData.LightningData);
		expect(decode((block?.data as LightningDataT).invoice)).toBe(validDescriptionHashBolt11);
	});

	it('narrows serialized Lightning data without unpacking the content block', async () => {
		const [block] = await parseContent(validBolt11);
		const builder = new flatbuffers.Builder(256);
		builder.finish(block!.pack(builder));

		const view = ContentBlock.getRootAsContentBlock(
			new flatbuffers.ByteBuffer(builder.asUint8Array())
		);
		const lightning = asLightningData(view);

		expect(lightning?.invoice()).toBe(validBolt11);
	});

	it('does not treat an LNURL as a BOLT11 invoice', async () => {
		const [block] = await parseContent(`lnurl1${'q'.repeat(117)}`);

		expect(block?.dataType).toBe(ContentData.NONE);
		expect(decode(block?.type ?? null)).toBe('text');
	});

	it.each([
		`${validBolt11.slice(0, -1)}q`,
		`L${validBolt11.slice(1)}`,
		invalidDescriptionHashLength,
		invalidImpreciseAmount,
		invalidZeroRScalar
	])('rejects an invalid BOLT11 candidate: %s', async (content) => {
		const [block] = await parseContent(content);

		expect(block?.dataType).toBe(ContentData.NONE);
		expect(decode(block?.type ?? null)).toBe('text');
		expect(decode(block?.text ?? null)).toBe(content);
	});

	it.each([`x${validBolt11}`, `${validBolt11}i`, `lnbc1${'q'.repeat(116)}\u212a`])(
		'does not extract an invoice from a larger or malformed token: %s',
		async (content) => {
			const [block] = await parseContent(content);

			expect(block?.dataType).toBe(ContentData.NONE);
			expect(decode(block?.type ?? null)).toBe('text');
			expect(decode(block?.text ?? null)).toBe(content);
		}
	);

	it.each([
		[`https://example.test/${validBolt11}`, ['link']],
		[`#${validBolt11}`, ['hashtag']],
		[`nostr:${validBolt11}`, ['text', 'lightning']]
	])('applies stable precedence to %s', async (content, expectedTypes) => {
		const blocks = await parseContent(content);

		expect(blocks.map((block) => decode(block.type))).toEqual(expectedTypes);
		expect(blocks.map((block) => decode(block.text)).join('')).toBe(content);
	});
});
