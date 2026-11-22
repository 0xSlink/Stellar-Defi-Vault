/**
 * Optional fee-bump sponsorship mode for the example vault frontend.
 *
 * When `sponsorshipMode === "fee-bump"`, the user signs the inner deposit
 * transaction and sends its base64 XDR to a sponsor relayer endpoint, which
 * wraps it in a `FeeBumpTransaction` so the user needs 0 XLM for fees.
 * When `sponsorshipMode === "none"`, the transaction is submitted directly.
 */

import {
  Contract,
  Networks,
  Transaction,
  TransactionBuilder,
  nativeToScVal,
  rpc,
} from "@stellar/stellar-sdk";

export type SponsorshipMode = "none" | "fee-bump";

export interface DepositRequest {
  contractId: string;
  userPublicKey: string;
  amount: bigint;
  rpcUrl: string;
  networkPassphrase?: string;
  sponsorshipMode: SponsorshipMode;
  sponsorEndpoint?: string;
  signTransaction: (xdr: string) => Promise<string>;
}

export interface DepositResult {
  mode: SponsorshipMode;
  hash: string;
}

export async function buildAndSubmitDeposit(
  req: DepositRequest,
): Promise<DepositResult> {
  const networkPassphrase = req.networkPassphrase ?? Networks.TESTNET;
  const server = new rpc.Server(req.rpcUrl);
  const account = await server.getAccount(req.userPublicKey);
  const contract = new Contract(req.contractId);

  const rawTx = new TransactionBuilder(account, {
    fee: "100",
    networkPassphrase,
  })
    .addOperation(
      contract.call(
        "deposit",
        nativeToScVal(req.userPublicKey, { type: "address" }),
        nativeToScVal(req.amount, { type: "i128" }),
      ),
    )
    .setTimeout(60)
    .build();

  const prepared = await server.prepareTransaction(rawTx);
  const signedInnerXdr = await req.signTransaction(prepared.toXDR());

  if (req.sponsorshipMode === "fee-bump") {
    if (!req.sponsorEndpoint) {
      throw new Error("sponsorEndpoint is required when sponsorshipMode is 'fee-bump'");
    }
    const response = await fetch(req.sponsorEndpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        innerTransactionXdr: signedInnerXdr,
        contractId: req.contractId,
      }),
    });
    if (!response.ok) {
      const detail = await response.text();
      throw new Error(`Sponsor relayer rejected request (${response.status}): ${detail}`);
    }
    const payload = (await response.json()) as { hash: string };
    return { mode: "fee-bump", hash: payload.hash };
  }

  const signedTx = TransactionBuilder.fromXDR(signedInnerXdr, networkPassphrase) as Transaction;
  const sent = await server.sendTransaction(signedTx);
  if (sent.status === "ERROR") {
    throw new Error(`Transaction submission failed: ${JSON.stringify(sent.errorResult)}`);
  }
  return { mode: "none", hash: sent.hash };
}
