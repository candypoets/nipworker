// Define the structure of a NIP-65 relay record
export interface RelayInfo {
	url: string;
	read: boolean;
	write: boolean;
}

export type Kind10002Parsed = RelayInfo[];
