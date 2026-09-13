// Test support for the signatures contract: the three answers of
// shared/signatures.ts that move into the core, as plain strings in and
// out. Shared by the contract test and the one-off measure test; nothing in
// the app imports this.
import { applySignature, signatureIn, stripSignature } from '@aperio/shared';

export interface ApplyInput {
  description: string;
  body: string;
}

export function answerApply(input: ApplyInput): string {
  return applySignature(input.description, input.body);
}

export interface TextInput {
  description: string;
}

export function answerStrip(input: TextInput): string {
  return stripSignature(input.description);
}

export function answerRead(input: TextInput): string | null {
  return signatureIn(input.description);
}
