import type { Proof } from "./proofs";

export type Kind9321Parsed = {
  amount: number;
  recipient: string;
  eventId?: string; // event being zapped if any
  mintUrl: string; // mint for the proofs
  redeemed: false; // Default to not redeemed, will check later if needed
  proofs: Proof[];
};
