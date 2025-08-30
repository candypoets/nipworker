// scripts/patch-strings.js
const fs = require("fs");
const path = require("path");

const genDir = "packages/nipworker/src/generated";

// Helper: walk directories recursively
function walk(dir, callback) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(fullPath, callback);
    } else if (entry.isFile() && entry.name.endsWith(".ts")) {
      callback(fullPath);
    }
  }
}

walk(genDir, (fpath) => {
  let code = fs.readFileSync(fpath, "utf8");

  // Replace calls to __string(...) with __stringByteString(...)
  code = code.replace(/__string\(/g, "__stringByteString(");

  // Replace return annotation ":string" → ": ByteString"
  code = code.replace(/:string/g, ": ByteString");

  // Replace union annotations like "string|" → "ByteString|"
  code = code.replace(/string\|/g, "ByteString|");

  // Replace union annotations like "|string" → "|ByteString"
  code = code.replace(/\|string/g, "|ByteString");

  // Ensure ByteString import (idempotent)
  if (!code.includes('import { ByteString } from "src/lib/ByteString"')) {
    code = `import { ByteString } from "src/lib/ByteString";\n` + code;
  }

  fs.writeFileSync(fpath, code, "utf8");
});
