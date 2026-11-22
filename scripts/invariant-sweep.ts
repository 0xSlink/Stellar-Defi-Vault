#!/usr/bin/env tsx
/**
 * Pre-release invariant sweep against a deployed VaultContract instance.
 *
 * Runs on-chain read-only health and solvency checks (`pool_health_report`,
 * `get_solvency_report`, `assert_invariants`, `vault_state`) via Soroban RPC,
 * then paginates contract event history to recompute total shares and
 * principal off-chain and compares them against on-chain contract state.
 *
 * Usage:
 *   CONTRACT_ID=C... RPC_URL=https://soroban-testnet.stellar.org \
 *     npx tsx scripts/invariant-sweep.ts [--start-ledger <n>] [--json]
 *
 * Exit codes:
 *   0 - All on-chain and off-chain invariant checks passed.
 *   1 - At least one invariant or health/solvency check failed.
 *   2 - Configuration, argument, or RPC communication error.
 */

import {
  Account,
  Contract,
  Keypair,
  Networks,
  TransactionBuilder,
  rpc,
  scValToNative,
  xdr,
} from "@stellar/stellar-sdk";

interface SweepArgs {
  contractId: string;
  rpcUrl: string;
  networkPassphrase: string;
  sourceAccount: string;
  startLedger?: number;
  json: boolean;
  help: boolean;
}

interface CheckResult {
  name: string;
  passed: boolean;
  expected: string;
  actual: string;
  details: string;
}

interface PoolHealthReport {
  reward_token_balance: bigint;
  total_staked: bigint;
  total_stakers: number;
  total_rewards_paid: bigint;
  reward_rate_bps: bigint;
  is_paused: boolean;
  is_stopped: boolean;
  uptime_ledgers: number;
  estimated_daily_obligations: bigint;
  is_solvent_7_days: boolean;
}

interface SolvencyReport {
  token_balance: bigint;
  total_principal: bigint;
  reward_pool: bigint;
  accrued_unclaimed: bigint;
  surplus_or_deficit: bigint;
}

interface EventRecomputation {
  eventsScanned: number;
  depositCount: number;
  withdrawCount: number;
  emergencyWithdrawCount: number;
  recomputedShares: bigint;
  recomputedPrincipal: bigint;
  firstLedger: number | null;
  lastLedger: number | null;
}

function parseArgs(argv: string[]): SweepArgs {
  const args: SweepArgs = {
    contractId: process.env.CONTRACT_ID?.trim() ?? "",
    rpcUrl:
      process.env.RPC_URL?.trim() ??
      process.env.STELLAR_RPC_URL?.trim() ??
      "https://soroban-testnet.stellar.org",
    networkPassphrase:
      process.env.NETWORK_PASSPHRASE?.trim() ??
      process.env.STELLAR_NETWORK_PASSPHRASE?.trim() ??
      Networks.TESTNET,
    sourceAccount:
      process.env.SOURCE_ACCOUNT?.trim() ?? Keypair.random().publicKey(),
    startLedger: process.env.START_LEDGER
      ? Number(process.env.START_LEDGER)
      : undefined,
    json: false,
    help: false,
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = (): string => {
      const val = argv[++i];
      if (val === undefined) {
        throw new Error(`Missing value for ${arg}`);
      }
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
      case "--source-account":
        args.sourceAccount = next().trim();
        break;
      case "--start-ledger": {
        const parsed = Number(next());
        if (!Number.isSafeInteger(parsed) || parsed < 1) {
          throw new Error("--start-ledger must be a positive integer");
        }
        args.startLedger = parsed;
        break;
      }
      case "--json":
        args.json = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return args;
}

function printHelp(): void {
  console.log(`Pre-release invariant sweep against a deployed VaultContract

Usage:
  npx tsx scripts/invariant-sweep.ts [options]

Options:
  --contract-id <id>          Deployed vault contract ID (or $CONTRACT_ID)
  --rpc-url <url>             Soroban RPC URL (or $RPC_URL, default testnet)
  --network-passphrase <p>    Stellar network passphrase (or $NETWORK_PASSPHRASE)
  --source-account <G...>     Public key for simulation source (or $SOURCE_ACCOUNT)
  --start-ledger <n>          Start ledger for event scan (or $START_LEDGER)
  --json                      Print JSON report instead of ASCII table
  -h, --help                  Show this help message
`);
}

function toBigInt(value: unknown): bigint {
  if (typeof value === "bigint") return value;
  if (typeof value === "number" && Number.isSafeInteger(value)) {
    return BigInt(value);
  }
  if (typeof value === "string" && /^-?\d+$/.test(value)) {
    return BigInt(value);
  }
  throw new Error(`Expected integer-compatible value, got ${String(value)}`);
}

async function simulateCall<T>(
  server: rpc.Server,
  contract: Contract,
  sourceKey: string,
  networkPassphrase: string,
  method: string,
): Promise<T> {
  // Use a synthetic zero-sequence account for read-only simulation when the
  // caller doesn't supply a funded account.
  let account: Account;
  try {
    account = await server.getAccount(sourceKey);
  } catch {
    account = new Account(sourceKey, "0");
  }

  const tx = new TransactionBuilder(account, {
    fee: "100",
    networkPassphrase,
  })
    .addOperation(contract.call(method))
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim)) {
    throw new Error(`${method} simulation failed: ${sim.error}`);
  }
  if (!sim.result) {
    throw new Error(`${method} returned no result`);
  }
  return scValToNative(sim.result.retval) as T;
}

function decodeScVal(value: unknown): unknown {
  if (value instanceof xdr.ScVal) {
    return scValToNative(value);
  }
  if (typeof value === "string") {
    return scValToNative(xdr.ScVal.fromXDR(value, "base64"));
  }
  if (typeof value === "object" && value !== null && "toXDR" in value) {
    return scValToNative(value as xdr.ScVal);
  }
  throw new Error("Malformed ScVal in contract event");
}

async function recomputeFromEvents(
  server: rpc.Server,
  contractId: string,
  startLedger: number,
): Promise<EventRecomputation> {
  let cursor: string | undefined;
  let eventsScanned = 0;
  let depositCount = 0;
  let withdrawCount = 0;
  let emergencyWithdrawCount = 0;
  let recomputedShares = 0n;
  let recomputedPrincipal = 0n;
  let firstLedger: number | null = null;
  let lastLedger: number | null = null;

  for (;;) {
    const response = await server.getEvents({
      ...(cursor ? { cursor } : { startLedger }),
      filters: [{ type: "contract", contractIds: [contractId] }],
      limit: 200,
    });

    const events = response.events ?? [];
    if (events.length === 0) {
      break;
    }

    for (const ev of events) {
      eventsScanned++;
      if (firstLedger === null || ev.ledger < firstLedger) {
        firstLedger = ev.ledger;
      }
      if (lastLedger === null || ev.ledger > lastLedger) {
        lastLedger = ev.ledger;
      }

      const topics = (ev.topic ?? []).map((t) => String(decodeScVal(t)));
      const eventName = topics[0];
      const payload = decodeScVal(ev.value);
      const data = Array.isArray(payload) ? payload : [payload];

      if (eventName === "deposit") {
        // data: [amount, shares_minted, ledger]
        depositCount++;
        const amount = toBigInt(data[0]);
        const sharesMinted = toBigInt(data[1]);
        recomputedPrincipal += amount;
        recomputedShares += sharesMinted;
      } else if (eventName === "withdraw") {
        // data: [shares_burned, amount_returned, ledger]
        withdrawCount++;
        const sharesBurned = toBigInt(data[0]);
        const amountReturned = toBigInt(data[1]);
        recomputedShares -= sharesBurned;
        recomputedPrincipal -= amountReturned;
      } else if (eventName === "emg_wdraw") {
        // data: [amount_returned, rewards_forfeited, ledger]
        emergencyWithdrawCount++;
        const amountReturned = toBigInt(data[0]);
        recomputedPrincipal -= amountReturned;
      }
    }

    const lastEvent = events[events.length - 1];
    if (!lastEvent?.pagingToken || events.length < 200) {
      break;
    }
    cursor = lastEvent.pagingToken;
  }

  return {
    eventsScanned,
    depositCount,
    withdrawCount,
    emergencyWithdrawCount,
    recomputedShares,
    recomputedPrincipal,
    firstLedger,
    lastLedger,
  };
}

function formatTable(results: CheckResult[]): string {
  const headers = ["Check", "Status", "Expected", "Actual", "Details"];
  const rows = results.map((r) => [
    r.name,
    r.passed ? "PASS" : "FAIL",
    r.expected,
    r.actual,
    r.details,
  ]);

  const widths = headers.map((h, idx) =>
    Math.max(h.length, ...rows.map((row) => row[idx]!.length)),
  );

  const formatRow = (cols: string[]): string =>
    "| " + cols.map((c, i) => c.padEnd(widths[i]!)).join(" | ") + " |";

  const separator =
    "+-" + widths.map((w) => "-".repeat(w)).join("-+-") + "-+";

  return [
    separator,
    formatRow(headers),
    separator,
    ...rows.map(formatRow),
    separator,
  ].join("\n");
}

async function runSweep(args: SweepArgs): Promise<{
  checks: CheckResult[];
  events: EventRecomputation;
  passed: boolean;
}> {
  const server = new rpc.Server(args.rpcUrl);
  const contract = new Contract(args.contractId);
  const checks: CheckResult[] = [];

  // 1. On-chain assert_invariants()
  try {
    await simulateCall<unknown>(
      server,
      contract,
      args.sourceAccount,
      args.networkPassphrase,
      "assert_invariants",
    );
    checks.push({
      name: "onchain_assert_invariants",
      passed: true,
      expected: "Ok(())",
      actual: "Ok(())",
      details: "Core on-chain accounting invariants passed",
    });
  } catch (err) {
    checks.push({
      name: "onchain_assert_invariants",
      passed: false,
      expected: "Ok(())",
      actual: "Error",
      details: (err as Error).message,
    });
  }

  // 2. On-chain pool_health_report()
  const health = await simulateCall<PoolHealthReport>(
    server,
    contract,
    args.sourceAccount,
    args.networkPassphrase,
    "pool_health_report",
  );
  checks.push({
    name: "pool_health_not_paused_or_stopped",
    passed: !health.is_paused && !health.is_stopped,
    expected: "paused=false, stopped=false",
    actual: `paused=${health.is_paused}, stopped=${health.is_stopped}`,
    details: `Uptime ${health.uptime_ledgers} ledgers, stakers=${health.total_stakers}`,
  });
  checks.push({
    name: "pool_health_7d_reward_solvency",
    passed: health.is_solvent_7_days,
    expected: "is_solvent_7_days=true",
    actual: `is_solvent_7_days=${health.is_solvent_7_days}`,
    details: `reward_balance=${toBigInt(health.reward_token_balance)}, daily_obligations=${toBigInt(health.estimated_daily_obligations)}`,
  });

  // 3. On-chain get_solvency_report()
  const solvency = await simulateCall<SolvencyReport>(
    server,
    contract,
    args.sourceAccount,
    args.networkPassphrase,
    "get_solvency_report",
  );
  const surplusOrDeficit = toBigInt(solvency.surplus_or_deficit);
  const tokenBalance = toBigInt(solvency.token_balance);
  const totalPrincipal = toBigInt(solvency.total_principal);
  checks.push({
    name: "solvency_non_negative_surplus",
    passed: surplusOrDeficit >= 0n,
    expected: ">= 0",
    actual: surplusOrDeficit.toString(),
    details: `held=${tokenBalance}, principal=${totalPrincipal}, accrued=${toBigInt(solvency.accrued_unclaimed)}`,
  });

  // 4. On-chain vault_state()
  const vaultState = await simulateCall<[bigint, bigint]>(
    server,
    contract,
    args.sourceAccount,
    args.networkPassphrase,
    "vault_state",
  );
  const onChainShares = toBigInt(vaultState[0]);
  const onChainPrincipal = toBigInt(vaultState[1]);

  // 5. Off-chain event history recomputation
  const latestLedger = (await server.getLatestLedger()).sequence;
  const startLedger = args.startLedger ?? Math.max(1, latestLedger - 17_280);
  const events = await recomputeFromEvents(server, args.contractId, startLedger);

  checks.push({
    name: "event_recompute_total_shares",
    passed: events.recomputedShares === onChainShares,
    expected: onChainShares.toString(),
    actual: events.recomputedShares.toString(),
    details: `Scanned ${events.eventsScanned} events (${events.depositCount} deposits, ${events.withdrawCount} withdrawals)`,
  });
  checks.push({
    name: "event_recompute_total_principal",
    passed: events.recomputedPrincipal === onChainPrincipal,
    expected: onChainPrincipal.toString(),
    actual: events.recomputedPrincipal.toString(),
    details: `Start ledger ${startLedger}..${latestLedger}`,
  });

  const passed = checks.every((c) => c.passed);
  return { checks, events, passed };
}

async function main(): Promise<void> {
  let args: SweepArgs;
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
    printHelp();
    process.exitCode = 2;
    return;
  }

  try {
    const result = await runSweep(args);
    if (args.json) {
      console.log(
        JSON.stringify(
          result,
          (_k, v: unknown) => (typeof v === "bigint" ? v.toString() : v),
          2,
        ),
      );
    } else {
      console.log(`Invariant Sweep — Contract ${args.contractId}`);
      console.log(`RPC: ${args.rpcUrl}`);
      console.log(formatTable(result.checks));
      console.log(
        result.passed
          ? "RESULT: ALL INVARIANT CHECKS PASSED"
          : "RESULT: INVARIANT SWEEP FAILED",
      );
    }

    if (!result.passed) {
      process.exitCode = 1;
    }
  } catch (err) {
    console.error(`Invariant sweep error: ${(err as Error).message}`);
    process.exitCode = 2;
  }
}

void main();
