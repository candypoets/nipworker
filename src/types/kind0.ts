export interface Kind0Parsed {
  pubkey?: string;
  name?: string;
  display_name?: string;
  picture?: string;
  banner?: string;
  about?: string;
  website?: string;
  nip05?: string; // NIP-05 verification (user@domain.com)
  lud06?: string; // Lightning Address (LNURL)
  lud16?: string; // Lightning Address (user@domain.com)

  // Contact information
  github?: string;
  twitter?: string;
  mastodon?: string;
  nostr?: string; // Preferred relay in NIP-05

  // Additional fields that may be present
  displayName?: string; // Alternative format
  username?: string; // Alternative to name
  bio?: string; // Alternative to about
  image?: string; // Alternative to picture
  avatar?: string; // Alternative to picture
  background?: string; // Alternative to banner

  // Any custom fields
  [key: string]: string | undefined;
}
