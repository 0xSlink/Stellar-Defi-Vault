/**
 * Mercury Subscription Setup
 *
 * Demonstrates how to register webhooks/subscriptions with Mercury's API
 * to capture real-time contract events from the Stellar DeFi Vault.
 */

export interface MercurySubscriptionConfig {
  apiKey: string;
  mercuryUrl: string;
  contractId: string;
  webhookUrl: string;
}

export async function createVaultSubscription(config: MercurySubscriptionConfig) {
  const query = `
    mutation SubscribeContractEvents($contractId: String!, $webhookUrl: String!) {
      createContractSubscription(
        contractId: $contractId,
        webhookUrl: $webhookUrl,
        topics: ["deposit", "withdraw", "claim", "vault"]
      ) {
        subscriptionId
        status
        createdAt
      }
    }
  `;

  const response = await fetch(config.mercuryUrl, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${config.apiKey}`,
    },
    body: JSON.stringify({
      query,
      variables: {
        contractId: config.contractId,
        webhookUrl: config.webhookUrl,
      },
    }),
  });

  const data = await response.json();
  if (data.errors) {
    throw new Error(`Mercury subscription failed: ${JSON.stringify(data.errors)}`);
  }
  return data.data.createContractSubscription;
}
