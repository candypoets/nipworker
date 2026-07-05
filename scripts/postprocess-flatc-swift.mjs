import { readFileSync, writeFileSync } from 'node:fs';

const file = process.argv[2];

if (!file) {
  console.error('Usage: node scripts/postprocess-flatc-swift.mjs <generated-swift-file>');
  process.exit(1);
}

let source = readFileSync(file, 'utf8');
const marker = 'public struct nostr_fb_ParsedDataUnion {';
const start = source.indexOf(marker);

if (start !== -1) {
  let depth = 0;
  let end = -1;

  for (let index = start; index < source.length; index += 1) {
    const char = source[index];
    if (char === '{') depth += 1;
    if (char === '}') {
      depth -= 1;
      if (depth === 0) {
        end = index + 1;
        break;
      }
    }
  }

  if (end === -1) {
    console.error(`Could not find end of ${marker}`);
    process.exit(1);
  }

  source =
    source.slice(0, start) +
    '// Removed by postprocess-flatc-swift: flatc emits a duplicate ParsedDataUnion object wrapper that collides with the enum.\n' +
    source.slice(end).replace(/^\n+/, '\n');
}

writeFileSync(file, source);
