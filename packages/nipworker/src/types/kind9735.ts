export type Kind9735Parsed = {
	id: string;
	amount: number; // Amount in sats
	content: string; // Content from the zap request
	bolt11: string; // Lightning invoice
	preimage?: string; // Payment preimage (optional)
	sender: string; // Pubkey of sender
	recipient: string; // Pubkey of recipient
	event?: string; // ID of the event being zapped (if any)
	eventCoordinate?: string; // Event coordinate for addressable events (if any)
	timestamp: number; // When the zap was created
	valid: boolean; // Whether the zap appears valid
	description: any; // The original zap request data
};
