export interface Contact {
	pubkey: string;
	relays?: string[];
	petname?: string;
}

export type Kind3Parsed = Contact[];
