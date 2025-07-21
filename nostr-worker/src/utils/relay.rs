use std::collections::HashSet;

/// Utility struct for relay URL handling and normalization
pub struct RelayUtils;

impl RelayUtils {
    /// Clean and normalize a list of relay URLs, removing duplicates and invalid entries
    pub fn clean_relays(relays: &Vec<String>) -> Vec<String> {
        relays
            .into_iter()
            .filter_map(|relay| {
                let normalized = Self::normalize_url(&relay);
                if normalized.is_empty() {
                    None
                } else {
                    Some(normalized)
                }
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Normalize a relay URL by ensuring proper protocol and format
    pub fn normalize_url(url: &str) -> String {
        let url = url.trim();
        if url.is_empty() {
            return String::new();
        }

        // Basic URL normalization
        if url.starts_with("wss://") || url.starts_with("ws://") {
            url.to_string()
        } else if url.starts_with("//") {
            format!("wss:{}", url)
        } else {
            format!("wss://{}", url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_url() {
        assert_eq!(
            RelayUtils::normalize_url("relay.example.com"),
            "wss://relay.example.com"
        );
        assert_eq!(
            RelayUtils::normalize_url("wss://relay.example.com"),
            "wss://relay.example.com"
        );
        assert_eq!(
            RelayUtils::normalize_url("ws://relay.example.com"),
            "ws://relay.example.com"
        );
        assert_eq!(
            RelayUtils::normalize_url("//relay.example.com"),
            "wss://relay.example.com"
        );
        assert_eq!(RelayUtils::normalize_url(""), "");
        assert_eq!(RelayUtils::normalize_url("   "), "");
    }

    #[test]
    fn test_clean_relays() {
        let relays = vec![
            "relay1.example.com".to_string(),
            "wss://relay2.example.com".to_string(),
            "//relay3.example.com".to_string(),
            "".to_string(),
            "relay1.example.com".to_string(), // duplicate
            "   ".to_string(),
        ];

        let cleaned = RelayUtils::clean_relays(&relays);

        // Should contain 3 unique, normalized URLs
        assert_eq!(cleaned.len(), 3);
        assert!(cleaned.contains(&"wss://relay1.example.com".to_string()));
        assert!(cleaned.contains(&"wss://relay2.example.com".to_string()));
        assert!(cleaned.contains(&"wss://relay3.example.com".to_string()));
    }

    #[test]
    fn test_clean_relays_empty() {
        let relays = vec!["".to_string(), "   ".to_string()];
        let cleaned = RelayUtils::clean_relays(&relays);
        assert!(cleaned.is_empty());
    }

    #[test]
    fn test_clean_relays_removes_duplicates() {
        let relays = vec![
            "relay.example.com".to_string(),
            "wss://relay.example.com".to_string(), // should be seen as duplicate after normalization
        ];

        let cleaned = RelayUtils::clean_relays(&relays);
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0], "wss://relay.example.com");
    }
}
