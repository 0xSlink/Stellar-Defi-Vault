/**
 * Event Schema Mapping
 *
 * Transforms raw Mercury GraphQL/webhook event payloads into the typed
 * event schema documented in docs/EVENTS.md.
 */

export interface ParsedDepositEvent {
  type: "deposit";
  depositor: string;
  tokenAmount: bigint;
  sharesMinted: bigint;
  ledgerSequence: number;
  timestamp: string;
}

export interface ParsedWithdrawEvent {
  type: "withdraw";
  withdrawer: string;
  tokenAmount: bigint;
  sharesBurned: bigint;
  ledgerSequence: number;
  timestamp: string;
}

export function parseMercuryEvent(rawEvent: any): ParsedDepositEvent | ParsedWithdrawEvent | null {
  const topic = rawEvent.topics?.[0];
  const primaryAddress = rawEvent.topics?.[1];
  const data = rawEvent.data;

  if (topic === "deposit") {
    return {
      type: "deposit",
      depositor: primaryAddress,
      tokenAmount: BigInt(data[0]),
      sharesMinted: BigInt(data[1]),
      ledgerSequence: Number(data[2]),
      timestamp: rawEvent.timestamp || new Date().toISOString(),
    };
  }

  if (topic === "withdraw") {
    return {
      type: "withdraw",
      withdrawer: primaryAddress,
      tokenAmount: BigInt(data[0]),
      sharesBurned: BigInt(data[1]),
      ledgerSequence: Number(data[2]),
      timestamp: rawEvent.timestamp || new Date().toISOString(),
    };
  }

  return null;
}
