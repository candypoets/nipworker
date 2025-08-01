import type { ProofUnion } from "./proofs";

export type Kind7375Parsed = {
  mintUrl: string;
  proofs: ProofUnion[];
  deletedIds?: string[]; // IDs of token events that were deleted
  decrypted: boolean; // Whether the content was successfully decrypted
};
