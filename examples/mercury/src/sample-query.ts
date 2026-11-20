/**
 * Sample Mercury GraphQL Query
 *
 * Queries historical user deposits from Mercury's indexed database.
 */

export const GET_USER_DEPOSIT_HISTORY = `
  query GetUserDeposits($contractId: String!, $userAddress: String!, $limit: Int) {
    contractEvents(
      contractId: $contractId,
      topic0: "deposit",
      topic1: $userAddress,
      limit: $limit
    ) {
      ledger
      timestamp
      data
      transactionHash
    }
  }
`;
