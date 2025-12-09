/**
 * Patch generated FlatBuffers TS files to use ByteString instead of string where appropriate.
 * - Converts calls to __string(...) into __stringByteString(...)
 * - Replaces ":string" with ": ByteString"
 * - Replaces "string|" with "ByteString|" and "|string" with "|ByteString"
 * - Ensures a single import of ByteString from "src/lib/ByteString"
 *
 * This script is ESM-compatible and targets the generated output in src/generated.
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Correct path to the generated code directory
const genDir = path.resolve(__dirname, '..', 'src', 'generated');

// Helper: walk directories recursively
function walk(dir, callback) {
	for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
		const fullPath = path.join(dir, entry.name);
		if (entry.isDirectory()) {
			walk(fullPath, callback);
		} else if (entry.isFile() && entry.name.endsWith('.ts')) {
			callback(fullPath);
		}
	}
}

if (!fs.existsSync(genDir) || !fs.statSync(genDir).isDirectory()) {
	console.error(`patch-strings: generated directory not found: ${genDir}`);
	process.exit(1);
}

walk(genDir, (fpath) => {
	let code = fs.readFileSync(fpath, 'utf8');

	// Replace calls to __string(...) with __stringByteString(...)
	code = code.replace(/__string\(/g, '__stringByteString(');

	// Replace return annotation ":string" → ": ByteString"
	code = code.replace(/:string/g, ': ByteString');

	// Replace union annotations like "string|" → "ByteString|"
	code = code.replace(/string\|/g, 'ByteString|');

	// Replace union annotations like "|string" → "|ByteString"
	code = code.replace(/\|string/g, '|ByteString');

	// Ensure ByteString import (idempotent)
	if (!code.includes('import { ByteString } from "src/lib/ByteString"')) {
		code = `import { ByteString } from "src/lib/ByteString";\n` + code;
	}

	fs.writeFileSync(fpath, code, 'utf8');
});
