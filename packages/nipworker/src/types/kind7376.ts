export type HistoryTag = {
	name: string;
	value: string;
	relay?: string;
	marker?: string;
};

// Kind7376Parsed represents a parsed spending history event
export type Kind7376Parsed = {
	direction: string; // "in" or "out"
	amount: number; // Amount in sats
	createdEvents: string[]; // IDs of token events created
	destroyedEvents: string[]; // IDs of token events destroyed
	redeemedEvents: string[]; // IDs of NIP-61 nutzap events redeemed
	tags?: HistoryTag[]; // All decrypted tags
	decrypted: boolean; // Whether content was successfully decrypted
};
