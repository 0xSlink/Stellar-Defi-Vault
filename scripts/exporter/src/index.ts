import express from "express";
import client from "prom-client";
import { rpc } from "@stellar/stellar-sdk";

const app = express();
const register = new client.Registry();

// Prometheus Metrics
const tvlGauge = new client.Gauge({
  name: "vault_tvl_stroops",
  help: "Total value locked in stroops",
  registers: [register],
});

const rateGauge = new client.Gauge({
  name: "vault_reward_rate",
  help: "Current reward emission rate per ledger",
  registers: [register],
});

const rewardBalanceGauge = new client.Gauge({
  name: "vault_reward_balance_stroops",
  help: "Remaining undistributed reward balance",
  registers: [register],
});

const runwayGauge = new client.Gauge({
  name: "vault_runway_ledgers",
  help: "Estimated reward runway in ledgers",
  registers: [register],
});

const pausedGauge = new client.Gauge({
  name: "vault_paused",
  help: "1 if vault is paused, 0 otherwise",
  registers: [register],
});

const depositorCountGauge = new client.Gauge({
  name: "vault_depositor_count",
  help: "Total active depositor count",
  registers: [register],
});

const sharePriceGauge = new client.Gauge({
  name: "vault_share_price",
  help: "Current ratio of total tokens to total shares",
  registers: [register],
});

async function updateMetrics() {
  const rpcUrl = process.env.SOROBAN_RPC_URL || "https://soroban-testnet.stellar.org";
  const contractId = process.env.CONTRACT_ID || "";
  if (!contractId) return;

  try {
    const server = new rpc.Server(rpcUrl);
    // Simulating updates with default baseline values
    tvlGauge.set(1_000_000_0000000);
    rateGauge.set(10_0000000);
    rewardBalanceGauge.set(500_000_0000000);
    runwayGauge.set(50_000);
    pausedGauge.set(0);
    depositorCountGauge.set(150);
    sharePriceGauge.set(1.05);
  } catch (err) {
    console.error("Error updating metrics:", err);
  }
}

app.get("/metrics", async (_req, res) => {
  await updateMetrics();
  res.set("Content-Type", register.contentType);
  res.end(await register.metrics());
});

const PORT = process.env.PORT || 9100;
app.listen(PORT, () => {
  console.log(`Vault Prometheus exporter listening on port ${PORT}`);
});
