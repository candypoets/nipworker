use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Hash a string to a fixed-length string
pub fn hash_string(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:016x}", hash)
}

/// Shorten subscription ID if it's too long
pub fn shorten_subscription_id(id: &str) -> String {
    if id.len() <= 64 {
        id.to_string()
    } else {
        hash_string(id)
    }
}
