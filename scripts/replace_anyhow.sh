#!/bin/bash

# Script to replace anyhow with specific error types in rust-worker

cd packages/rust-worker/src

# Replace anyhow imports in parser files with ParserError
for file in parser/*.rs; do
    if grep -q "use anyhow::" "$file"; then
        echo "Processing parser file: $file"
        sed -i '' 's/use anyhow::{anyhow, Result};/use crate::parser::{ParserError, Result};/' "$file"
        sed -i '' 's/use anyhow::Result;/use crate::parser::Result;/' "$file"
        sed -i '' 's/use anyhow::anyhow;/use crate::parser::ParserError;/' "$file"
        # Replace anyhow! macro with ParserError
        sed -i '' 's/anyhow!(\([^)]*\))/ParserError::Other(\1.to_string())/' "$file"
        sed -i '' 's/return Err(anyhow::anyhow!(\([^)]*\)))/return Err(ParserError::Other(\1.to_string()))/' "$file"
    fi
done

# Replace anyhow imports in network files with NostrError
for file in network/*.rs; do
    if grep -q "use anyhow::" "$file"; then
        echo "Processing network file: $file"
        sed -i '' 's/use anyhow::Result;/use crate::NostrError;\ntype Result<T> = std::result::Result<T, NostrError>;/' "$file"
        sed -i '' 's/use anyhow::anyhow;/use crate::NostrError;/' "$file"
        # Replace anyhow! macro with NostrError
        sed -i '' 's/anyhow!(\([^)]*\))/NostrError::Other(\1.to_string())/' "$file"
    fi
done

# Replace anyhow imports in signer files with SignerError
for file in signer/*.rs; do
    if grep -q "use anyhow::" "$file"; then
        echo "Processing signer file: $file"
        sed -i '' 's/use anyhow::Result;/use crate::signer::SignerError;\ntype Result<T> = std::result::Result<T, SignerError>;/' "$file"
        sed -i '' 's/use anyhow::anyhow;/use crate::signer::SignerError;/' "$file"
        # Replace anyhow! macro with SignerError
        sed -i '' 's/anyhow!(\([^)]*\))/SignerError::Other(\1.to_string())/' "$file"
    fi
done

# Replace anyhow in db/index.rs with DatabaseError
if grep -q "use anyhow::" "db/index.rs"; then
    echo "Processing db/index.rs"
    sed -i '' 's/use anyhow::Result;/use crate::db::types::DatabaseError;\ntype Result<T> = std::result::Result<T, DatabaseError>;/' "db/index.rs"
    sed -i '' 's/anyhow!(\([^)]*\))/DatabaseError::StorageError(\1.to_string())/' "db/index.rs"
fi

# Replace anyhow in types/nostr.rs
if grep -q "use anyhow::" "types/nostr.rs"; then
    echo "Processing types/nostr.rs"
    sed -i '' 's/use anyhow::{anyhow, Result};/use crate::types::TypesError;\ntype Result<T> = std::result::Result<T, TypesError>;/' "types/nostr.rs"
    sed -i '' 's/anyhow!(\([^)]*\))/TypesError::InvalidFormat(\1.to_string())/' "types/nostr.rs"
fi

# Replace anyhow in pipeline files
for file in pipeline/*.rs pipeline/pipes/*.rs; do
    if [[ -f "$file" ]] && grep -q "use anyhow::" "$file"; then
        echo "Processing pipeline file: $file"
        sed -i '' 's/use anyhow::{anyhow, Result};/use crate::NostrError;\ntype Result<T> = std::result::Result<T, NostrError>;/' "$file"
        sed -i '' 's/use anyhow::Result;/use crate::NostrError;\ntype Result<T> = std::result::Result<T, NostrError>;/' "$file"
        sed -i '' 's/anyhow!(\([^)]*\))/NostrError::Other(\1.to_string())/' "$file"
    fi
done

echo "Done! Now you need to:"
echo "1. Add 'Other(String)' variant to SignerError if not present"
echo "2. Check and fix any compilation errors"
echo "3. Remove anyhow from Cargo.toml"
