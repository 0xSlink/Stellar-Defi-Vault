#!/usr/bin/env tsx
/**
 * Release-Candidate (RC) testnet soak load generator and health monitor.
 *
 * Generates randomized user activity (deposits, reward claims, withdrawals)
 * against a deployed testnet RC vault while continuously verifying
 * `pool_health_report` and `get_solvency_report` stay green.
 *
 * Outputs a release-notes-ready summary at the end of the soak and exits
 * non-zero if any health/solvency check fails or the error rate exceeds
 * the configured threshold.
 *
 * Usage:
 *   CONTRACT_ID=C... USER_SECRETS=S...,S... \
 *     npx tsx scripts/soak-testnet.ts --duration-minutes 60
 *
 * Exit codes:
 *   0 - Soak completed with all health and solvency checks green.
 *   1 - Health/solvency check failed or error threshold breached during soak.
 *   2 - Configuration or fatal RPC error.
 */

import {
  Account,
  Contract,
  Keypair,
  Networks,
  TransactionBuilder,
  nativeToScVal,
  rpc,
  scValToNative,
} from "@stellar/stellar-sdk";

type ActionKind = "deposit" | "claim" | "withdraw";

interface SoakArgs {
  contractId: string;
  rpcUrl: string;
  networkPassphrase: string;
  userSecrets: string[];
  durationMinutes: number;
  intervalMs: number;
  healthIntervalMs: number;
  minAmount: bigint;
  maxAmount: bigint;
  maxErrorRatePct: number;
  help: boolean;
}

interface SoakStats {
  startedAt: string;
  endedAt: string;
  contractId: string;
  durationMinutes: number;
  totalAttempts: number;
  succeeded: number;
  failed: number;
  deposits: number;
  claims: number;
  withdrawals: number;
  healthChecksPassed: number;
  healthChecksFailed: number;
  lastError?: string;
}

function parseArgs(argv: string[]): SoakArgs {
  const secretsEnv = process.env.USER_SECRETS?.trim() ?? "";
  const args: SoakArgs = {
    contractId: process.env.CONTRACT_ID?.trim() ?? "",
    rpcUrl:
      process.env.RPC_URL?.trim() ??
      process.env.STELLAR_RPC_URL?.trim() ??
      "https://soroban-testnet.stellar.org",
    networkPassphrase:
      process.env.NETWORK_PASSPHRASE?.trim() ??
      process.env.STELLAR_NETWORK_PASSPHRASE?.trim() ??
      Networks.TESTNET,
    userSecrets: secretsEnv
      ? secretsEnv.split(",").map((s) => s.trim()).filter(Boolean)
      : [],
    durationMinutes: process.env.SOAK_DURATION_MINUTES
      ? Number(process.env.SOAK_DURATION_MINUTES)
      : 60,
    intervalMs: process.env.SOAK_INTERVAL_MS
      ? Number(process.env.SOAK_INTERVAL_MS)
      : 10_000,
    healthIntervalMs: process.env.HEALTH_INTERVAL_MS
      ? Number(process.env.HEALTH_INTERVAL_MS)
      : 60_000,
    minAmount: BigInt(process.env.MIN_AMOUNT ?? "1000000"),
    maxAmount: BigInt(process.env.MAX_AMOUNT ?? "10000000"),
    maxErrorRatePct: process.env.MAX_ERROR_RATE_PCT
      ? Number(process.env.MAX_ERROR_RATE_PCT)
      : 5,
    help: false,
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = (): string => {
      const val = argv[++i];
      if (val === undefined) throw new Error(`Missing value for ${arg}`);
      return val;
    };

    switch (arg) {
      case "-h":
      case "--help":
        args.help = true;
        break;
      case "--contract-id":
        args.contractId = next().trim();
        break;
      case "--rpc-url":
        args.rpcUrl = next().trim();
        break;
      case "--network-passphrase":
        args.networkPassphrase = next().trim();
        break;
      case "--user-secrets":
        args.userSecrets = next()
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
        break;
      case "--duration-minutes":
        args.durationMinutes = Number(next());
        break;
      case "--interval-ms":
        args.intervalMs = Number(next());
        break;
      case "--health-interval-ms":
        args.healthIntervalMs = Number(next());
        break;
      case "--min-amount":
        args.minAmount = BigInt(next());
        break;
      case "--max-amount":
        args.maxAmount = BigInt(next());
        break;
      case "--max-error-rate-pct":
        args.maxErrorRatePct = Number(next());
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return args;
}

function printHelp(): void {
  console.log(`Testnet Release-Candidate (RC) soak load generator

Usage:
  npx tsx scripts/soak-testnet.ts [options]

Options:
  --contract-id <id>          RC vault contract ID (or $CONTRACT_ID)
  --user-secrets <S1,S2,...>  Comma-separated testnet secret keys (or $USER_SECRETS)
  --rpc-url <url>             Soroban RPC URL (default testnet)
  --network-passphrase <p>    Stellar network passphrase (default testnet)
  --duration-minutes <n>      Soak duration in minutes (default 60)
  --interval-ms <n>           Delay between user operations in ms (default 10000)
  --health-interval-ms <n>    Health check polling interval in ms (default 60000)
  --min-amount <stroops>      Minimum deposit/withdraw amount (default 1000000)
  --max-amount <stroops>      Maximum deposit/withdraw amount (default 10000000)
  --max-error-rate-pct <n>    Maximum allowed tx error rate % (default 5)
  -h, --help                  Show this help message
`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function randomBigInt(min: bigint, max: bigint): bigint {
  if (max <= min) return min;
  const span = max - min + 1n;
  const offset = BigInt(Math.floor(Math.random() * Number(span)));
  return min + offset;
}

function pickAction(hasDeposited: boolean): ActionKind {
  if (!hasDeposited) return "deposit";
  const r = Math.random();
  if (r < 0.5) return "deposit";
  if (r < 0.8) return "claim";
  return "withdraw";
}

async function submitContractCall(
  server: rpc.Server,
  contract: Contract,
  signer: Keypair,
  networkPassphrase: string,
  method: string,
  ...params: ReturnType<typeof nativeToScVal>[]
): Promise<void> {
  const account = await server.getAccount(signer.publicKey());
  const tx = new TransactionBuilder(account, {
    fee: "100000",
    networkPassphrase,
  })
    .addOperation(contract.call(method, ...params))
    .setTimeout(30)
    .build();

  const prepared = await server.prepareTransaction(tx);
  prepared.sign(signer);

  const sent = await server.sendTransaction(prepared);
  if (sent.status === "ERROR") {
    throw new Error(`sendTransaction(${method}) failed: ${JSON.stringify(sent.errorResult)}`);
  }

  for (let attempt = 0; attempt < 20; attempt++) {
    const result = await server.getTransaction(sent.hash);
    if (result.status === "SUCCESS") return;
    if (result.status === "FAILED") {
      throw new Error(`Transaction ${sent.hash} (${method}) FAILED`);
    }
    await sleep(1_500);
  }
  throw new Error(`Timed out waiting for ${method} tx ${sent.hash}`);
}

async function checkHealthAndSolvency(
  server: rpc.Server,
  contract: Contract,
  probePublicKey: string,
  networkPassphrase: string,
): Promise<{ ok: boolean; reason?: string }> {
  let account: Account;
  try {
    account = await server.getAccount(probePublicKey);
  } catch {
    account = new Account(probePublicKey, "0");
  }

  const runReadOnly = async (method: string): Promise<Record<string, unknown>> => {
    const tx = new TransactionBuilder(account, { fee: "100", networkPassphrase })
      .addOperation(contract.call(method))
      .setTimeout(30)
      .build();
    const sim = await server.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(sim) || !sim.result) {
      throw new Error(`${method} simulation error`);
    }
    return scValToNative(sim.result.retval) as Record<string, unknown>;
  };

  const health = await runReadOnly("pool_health_report");
  if (health.is_paused === true || health.is_stopped === true) {
    return { ok: false, reason: "pool is paused or stopped" };
  }
  if (health.is_solvent_7_days === false) {
    return { ok: false, reason: "pool failed 7-day reward solvency check" };
  }

  const solvency = await runReadOnly("get_solvency_report");
  if (BigInt(String(solvency.surplus_or_deficit ?? 0)) < 0n) {
    return { ok: false, reason: `negative solvency surplus: ${String(solvency.surplus_or_deficit)}` };
  }

  return { ok: true };
}

function formatReleaseNotesSummary(stats: SoakStats, passed: boolean): string {
  const errorRate =
    stats.totalAttempts === 0
      ? "0.00%"
      : `${((stats.failed / stats.totalAttempts) * 100).toFixed(2)}%`;

  return [
    "### Testnet RC Soak Results",
    `- **Status**: ${passed ? "PASS" : "FAIL"}`,
    `- **Contract ID**: \`${stats.contractId}\``,
    `- **Window**: ${stats.startedAt} -> ${stats.endedAt} (${stats.durationMinutes} min)`,
    `- **Operations**: ${stats.totalAttempts} total (${stats.succeeded} ok, ${stats.failed} failed, error rate ${errorRate})`,
    `- **Breakdown**: ${stats.deposits} deposits, ${stats.claims} claims, ${stats.withdrawals} withdrawals`,
    `- **Health & Solvency Checks**: ${stats.healthChecksPassed} passed, ${stats.healthChecksFailed} failed`,
    ...(stats.lastError ? [`- **Last Error**: \`${stats.lastError}\``] : []),
  ].join("\n");
}

async function runSoak(args: SoakArgs): Promise<void> {
  const server = new rpc.Server(args.rpcUrl);
  const contract = new Contract(args.contractId);
  const signers = args.userSecrets.map((s) => Keypair.fromSecret(s));
  const depositedUsers = new Set<string>();

  const startedAt = new Date();
  const endTimeMs = startedAt.getTime() + args.durationMinutes * 60_000;
  let lastHealthCheckMs = 0;

  const stats: SoakStats = {
    startedAt: startedAt.toISOString(),
    endedAt: startedAt.toISOString(),
    contractId: args.contractId,
    durationMinutes: args.durationMinutes,
    totalAttempts: 0,
    succeeded: 0,
    failed: 0,
    deposits: 0,
    claims: 0,
    withdrawals: 0,
    healthChecksPassed: 0,
    healthChecksFailed: 0,
  };

  while (Date.now() < endTimeMs) {
    const signer = signers[Math.floor(Math.random() * signers.length)]!;
    const pubKey = signer.publicKey();
    const action = pickAction(depositedUsers.has(pubKey));
    stats.totalAttempts++;

    try {
      if (action === "deposit") {
        const amount = randomBigInt(args.minAmount, args.maxAmount);
        await submitContractCall(
          server,
          contract,
          signer,
          args.networkPassphrase,
          "deposit",
          nativeToScVal(pubKey, { type: "address" }),
          nativeToScVal(amount, { type: "i128" }),
        );
        depositedUsers.add(pubKey);
        stats.deposits++;
      } else if (action === "claim") {
        await submitContractCall(
          server,
          contract,
          signer,
          args.networkPassphrase,
          "claim_rewards",
          nativeToScVal(pubKey, { type: "address" }),
        );
        stats.claims++;
      } else {
        const amount = args.minAmount;
        await submitContractCall(
          server,
          contract,
          signer,
          args.networkPassphrase,
          "withdraw",
          nativeToScVal(pubKey, { type: "address" }),
          nativeToScVal(amount, { type: "i128" }),
        );
        stats.withdrawals++;
      }
      stats.succeeded++;
    } catch (err) {
      stats.failed++;
      stats.lastError = (err as Error).message;
      console.error(`[soak] ${action} error for ${pubKey}: ${stats.lastError}`);
    }

    if (Date.now() - lastHealthCheckMs >= args.healthIntervalMs) {
      lastHealthCheckMs = Date.now();
      try {
        const health = await checkHealthAndSolvency(
          server,
          contract,
          signers[0]!.publicKey(),
          args.networkPassphrase,
        );
        if (health.ok) {
          stats.healthChecksPassed++;
        } else {
          stats.healthChecksFailed++;
          stats.lastError = health.reason;
          console.error(`[soak] Health check failed: ${health.reason}`);
        }
      } catch (err) {
        stats.healthChecksFailed++;
        stats.lastError = (err as Error).message;
        console.error(`[soak] Health check RPC error: ${stats.lastError}`);
      }
    }

    await sleep(args.intervalMs);
  }

  stats.endedAt = new Date().toISOString();
  const errorRatePct =
    stats.totalAttempts === 0 ? 0 : (stats.failed / stats.totalAttempts) * 100;
  const passed =
    stats.healthChecksFailed === 0 && errorRatePct <= args.maxErrorRatePct;

  console.log("\n" + formatReleaseNotesSummary(stats, passed));
  if (!passed) {
    process.exitCode = 1;
  }
}

async function main(): Promise<void> {
  let args: SoakArgs;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (err) {
    console.error(`Argument error: ${(err as Error).message}`);
    printHelp();
    process.exitCode = 2;
    return;
  }

  if (args.help) {
    printHelp();
    return;
  }

  if (!args.contractId) {
    console.error("Error: --contract-id (or $CONTRACT_ID) is required.");
    process.exitCode = 2;
    return;
  }

  if (args.userSecrets.length === 0) {
    console.error("Error: --user-secrets (or $USER_SECRETS) must provide at least one testnet keypair.");
    process.exitCode = 2;
    return;
  }

  try {
    await runSoak(args);
  } catch (err) {
    console.error(`Soak fatal error: ${(err as Error).message}`);
    process.exitCode = 2;
  }
}

void main();
