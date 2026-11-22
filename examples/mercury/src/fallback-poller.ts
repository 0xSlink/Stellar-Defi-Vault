/**
 * RPC Polling Fallback
 *
 * Provides resilient fallback to Soroban RPC getEvents if Mercury API
 * reaches subscription limits, rate limits, or is temporarily unreachable.
 */

import { rpc } from "@stellar/stellar-sdk";

export async function pollEventsRpcFallback(
  rpcUrl: string,
  contractId: string,
  startLedger: number,
) {
  const server = new rpc.Server(rpcUrl);
  const response = await server.getEvents({
    startLedger,
    filters: [
      {
        type: "contract",
        contractIds: [contractId],
      },
    ],
  });
  return response.events;
}
