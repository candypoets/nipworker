export enum ReactionType {
	LIKE = '+',
	DISLIKE = '-',
	EMOJI = 'emoji',
	CUSTOM = 'custom'
}

export type Kind7Parsed = {
	type: ReactionType;
	eventId: string; // The id of the event being reacted to
	pubkey: string; // The pubkey of the author of the reacted event
	eventKind?: number; // The kind of the event being reacted to (from k tag)
	emoji?: {
		shortcode: string;
		url: string;
	};
	targetCoordinates?: string; // For addressable events (from a tag)
};
