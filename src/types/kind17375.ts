export type Kind17375Parsed = {
	mints: string[]; // List of mint URLs
	p2pkPrivKey?: string; // Private key for P2PK ecash (if decrypted)
	p2pkPubKey?: string;
	decrypted: boolean; // Whether content was successfully decrypted
};
