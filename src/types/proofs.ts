// Common proof types used across multiple kind parsers

// Represents a proof in the Cashu protocol
export type Proof = {
  id: string;
  C: string;
  secret: string;
  amount: number;
};

// Represents a v4 proof in the Cashu protocol
export type ProofV4 = {
  version: 4;
  id: string;
  amount: number;
  secret: string;
  C: string;
};

// Union type to handle both Proof and ProofV4
export type ProofUnion = Proof | ProofV4;

// Content structure for token events
export type TokenContent = {
  mint: string;
  proofs: ProofUnion[];
  id?: string;
  del?: string[];
};
