import { ContentBlock } from ".";

export type Kind4Parsed = {
  parsedContent?: ContentBlock[];
  decryptedContent?: string;
  chatID: string;
  recipient: string;
};
