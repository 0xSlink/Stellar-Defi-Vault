#!/usr/bin/env tsx
/**
 * vaultctl — a typed operator CLI for the Stellar DeFi Vault (issue #859).
 *
 * Replaces the ad hoc `stellar contract invoke ...` shell snippets operators
 * keep re-typing with one consistent, scriptable entry point.
 *
 * # Commands
 *
 *   status                  Pool health snapshot: TVL, reward rate, reward
 *                           pool, pause/insolvency flags, depositor count.
 *   solvency                Insolvency flag plus share/deposit totals and the
 *                           insurance-fund balance.
 *   config                  Dump the vault's live on-chain configuration.
 *   params diff             Compare on-chain parameters against a deployment
 *                           TOML config and list any mismatches.
 *   set-rate <bps>          Admin: set the reward rate (basis points).
 *   fund <amount>           Admin: top up the reward pool from an address.
 *   pause [reason] [msg]    Admin: pause the pool.
 *   unpause                 Admin: resume the pool.
 *
 * # Configuration
 *
 * Every value is read from a flag or an environment variable, so the CLI
 * drops into CI or a shell script unchanged:
 *
 *   CONTRACT_ID                         deployed vault contract id
 *   IDENTITY                            local Stellar identity (as --source)
 *   NETWORK                             named network (default: testnet)
 *   RPC_URL / STELLAR_RPC_URL           custom RPC endpoint
 *   NETWORK_PASSPHRASE / STELLAR_NETWORK_PASSPHRASE
 *   DECIMALS                            token decimals for display (default 7)
 *
 * # Safety
 *
 * Every mutating command is *simulated first* (`--send=no`); only once that
 * simulation succeeds does it submit (`--send=yes`). Pass `--dry-run` to stop
 * after the simulation and never submit. Writes against mainnet additionally
 * require an explicit `--i-understand` acknowledgement.
 *
 * # Output
 *
 * Human-readable tables by default; pass `--json` (or `--format json`) for a
 * machine-readable document. `scripts/verify-deployment.ts` imports the
 * comparison helpers here rather than re-implementing them.
 *
 * Usage:
 *   npx tsx scripts/vaultctl.ts status
 *   CONTRACT_ID=C... IDENTITY=ops npx tsx scripts/vaultctl.ts --json status
 *   npx tsx scripts/vaultctl.ts --dry-run set-rate 500
 *   npx tsx scripts/vaultctl.ts params diff --config config/testnet.toml
 */

import { execFileSync } from "node:child_process";
import * as fs from "node:fs";

// ─────────────────────────────── shared types ───────────────────────────────

export type OutputFormat = "table" | "json";

/** Everything a command needs to talk to a deployed vault. */
export interface CliContext {
  contractId: string;
  identity: string;
  network: string;
  rpcUrl?: string;
  networkPassphrase?: string;
  decimals: number;
  /** Simulate mutating calls but never submit them. */
  dryRun: boolean;
  /** Explicit acknowledgement required for mainnet writes. */
  iUnderstand: boolean;
  format: OutputFormat;
}

const MAINNET_PASSPHRASE = "Public Global Stellar Network ; September 2015";

/** True when the target network is Stellar pubnet/mainnet. */
export function isMainnet(ctx: CliContext): boolean {
  const name = ctx.network.toLowerCase();
  if (name === "mainnet" || name === "public" || name === "pubnet") return true;
  return ctx.networkPassphrase === MAINNET_PASSPHRASE;
}

// ─────────────────────────────── invoke helpers ─────────────────────────────

function baseCliArgs(ctx: CliContext): string[] {
  const args = ["contract", "invoke", "--id", ctx.contractId, "--source", ctx.identity];
  if (ctx.rpcUrl) {
    args.push("--rpc-url", ctx.rpcUrl);
    if (ctx.networkPassphrase) {
      args.push("--network-passphrase", ctx.networkPassphrase);
    }
  } else {
    args.push("--network", ctx.network);
  }
  return args;
}

/** Read-only call: simulates on-chain without ever submitting a transaction. */
export function simulate(ctx: CliContext, fn: string, fnArgs: string[] = []): string {
  const args = [...baseCliArgs(ctx), "--send=no", "--", fn, ...fnArgs];
  return execFileSync("stellar", args, { encoding: "utf8" }).trim();
}

/** Read-only call that returns `null` instead of throwing, for optional probes. */
export function trySimulate(ctx: CliContext, fn: string, fnArgs: string[] = []): string | null {
  try {
    return simulate(ctx, fn, fnArgs);
  } catch {
    return null;
  }
}

export interface SubmitResult {
  /** Output of the pre-flight simulation. */
  simulated: string;
  /** Whether the transaction was actually submitted. */
  submitted: boolean;
  /** Output of the submitted transaction (or the simulation under --dry-run). */
  result: string;
}

/**
 * Mutating call. Always simulates first, then submits unless `--dry-run`.
 * Refuses to write to mainnet without `--i-understand`.
 */
export function submit(ctx: CliContext, fn: string, fnArgs: string[] = []): SubmitResult {
  const simulated = simulate(ctx, fn, fnArgs);
  if (ctx.dryRun) {
    return { simulated, submitted: false, result: simulated };
  }
  if (isMainnet(ctx) && !ctx.iUnderstand) {
    throw new Error(
      "Refusing to submit a mainnet write without --i-understand. " +
        "Re-run with --i-understand once you have verified the simulation above.",
    );
  }
  const args = [...baseCliArgs(ctx), "--send=yes", "--", fn, ...fnArgs];
  const result = execFileSync("stellar", args, { encoding: "utf8" }).trim();
  return { simulated, submitted: true, result };
}

/** Resolves a local identity name to its public G... address. */
export function resolveIdentityAddress(identity: string): string {
  return execFileSync("stellar", ["keys", "address", identity], { encoding: "utf8" }).trim();
}

// ──────────────────────────────── parsing ───────────────────────────────────

export function parseNumbers(raw: string): string[] {
  return raw.match(/-?\d+/g) ?? [];
}

export function parseLastNumber(raw: string): string {
  const matches = parseNumbers(raw);
  if (matches.length === 0) {
    throw new Error(`Could not parse numeric contract output: ${raw}`);
  }
  return matches[matches.length - 1];
}

export function parseBool(raw: string): boolean {
  const cleaned = raw.trim().replace(/^"|"$/g, "").toLowerCase();
  if (cleaned === "true") return true;
  if (cleaned === "false") return false;
  throw new Error(`Could not parse boolean contract output: ${raw}`);
}

export function parseAddress(raw: string): string | null {
  const match = raw.match(/[GC][A-Z2-7]{55}/);
  return match ? match[0] : null;
}

// ──────────────────────────────── formatting ────────────────────────────────

/** Renders a raw base-unit integer as a fixed-point decimal string. */
export function formatUnits(raw: string, decimals: number): string {
  if (decimals <= 0) return raw;
  const negative = raw.startsWith("-");
  const digits = negative ? raw.slice(1) : raw;
  const padded = digits.padStart(decimals + 1, "0");
  const whole = padded.slice(0, padded.length - decimals);
  const fraction = padded.slice(padded.length - decimals);
  return `${negative ? "-" : ""}${whole}.${fraction}`;
}

export function printTable(title: string, rows: Array<[string, string]>): void {
  const width = rows.reduce((max, [key]) => Math.max(max, key.length), title.length);
  console.log(title);
  console.log("─".repeat(width + 2));
  for (const [key, value] of rows) {
    console.log(`${key.padEnd(width)}  ${value}`);
  }
}

function emit(ctx: CliContext, json: unknown, title: string, rows: Array<[string, string]>): void {
  if (ctx.format === "json") {
    console.log(JSON.stringify(json, null, 2));
    return;
  }
  printTable(title, rows);
}

// ────────────────────────── config-file comparison ──────────────────────────

/** Minimal TOML reader for the flat `[section]`/`key = value` config format. */
export function parseToml(content: string): Record<string, any> {
  const result: Record<string, any> = {};
  let currentSection = "";

  for (const line of content.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;

    const sectionMatch = trimmed.match(/^\[(.+)\]$/);
    if (sectionMatch) {
      currentSection = sectionMatch[1];
      result[currentSection] = {};
      continue;
    }

    const kvMatch = trimmed.match(/^(\w+)\s*=\s*(.+)$/);
    if (kvMatch && currentSection) {
      let [, key, value] = kvMatch;
      value = value.trim();

      const commentIndex = value.indexOf("#");
      if (commentIndex > 0) {
        value = value.substring(0, commentIndex).trim();
      }

      if (value.startsWith('"') && value.endsWith('"')) {
        result[currentSection][key] = value.slice(1, -1);
      } else if (value.startsWith("[") && value.endsWith("]")) {
        const items = value
          .slice(1, -1)
          .split(",")
          .map((s) => s.trim().replace(/"/g, ""));
        result[currentSection][key] = items.filter((s) => s.length > 0);
      } else if (value === "true" || value === "false") {
        result[currentSection][key] = value === "true";
      } else {
        const num = Number(value);
        result[currentSection][key] = isNaN(num) ? value : num;
      }
    }
  }

  return result;
}

export interface ParamSpec {
  /** Operator-facing label. */
  label: string;
  /** `<section>.<key>` path into the parsed config file. */
  path: string;
  /** On-chain read entrypoint used to fetch the live value. */
  read: string;
  kind: "number" | "address" | "bool";
}

/**
 * Config keys with a direct on-chain read. Kept deliberately small: only
 * parameters that have a matching public getter are compared, so the diff
 * never invents a mapping. `withdrawal_fee_bps`, `token_address` and friends
 * have no getter and are simply not compared.
 */
export const PARAM_SPECS: ParamSpec[] = [
  { label: "Admin", path: "admin.admin_addresses", read: "get_admin", kind: "address" },
  {
    label: "Performance fee (bps)",
    path: "vault.performance_fee_bps",
    read: "get_performance_fee_bps",
    kind: "number",
  },
  {
    label: "Max deposit cap",
    path: "vault.max_deposit_cap",
    read: "get_max_deposit_cap",
    kind: "number",
  },
  {
    label: "Base yield rate (bps)",
    path: "vault.base_yield_rate_bps",
    read: "get_reward_rate_bps",
    kind: "number",
  },
];

export interface ParamComparison {
  label: string;
  /** On-chain getter consulted. */
  read: string;
  expected: string;
  actual: string;
  passed: boolean;
}

function getAtPath(config: Record<string, any>, path: string): unknown {
  return path.split(".").reduce<any>((node, key) => (node == null ? undefined : node[key]), config);
}

function normalizeOnChain(raw: string | null, kind: ParamSpec["kind"]): string {
  if (raw === null) return "(unavailable)";
  switch (kind) {
    case "number":
      try {
        return parseLastNumber(raw);
      } catch {
        return raw;
      }
    case "bool":
      try {
        return String(parseBool(raw));
      } catch {
        return raw;
      }
    case "address":
      return parseAddress(raw) ?? raw.replace(/^"|"$/g, "");
  }
}

function formatExpected(value: unknown): string {
  if (value === undefined || value === null) return "";
  if (Array.isArray(value)) {
    return value.length > 0 ? String(value[0]) : "";
  }
  return String(value);
}

/**
 * Compares each `PARAM_SPECS` entry against the live contract. Entries whose
 * key is absent from the config file are skipped (`passed: true`,
 * expected `"(not configured)"`) so an intentionally partial config does not
 * raise false alarms.
 */
export function collectParamComparisons(
  ctx: CliContext,
  config: Record<string, any>,
): ParamComparison[] {
  return PARAM_SPECS.map((spec) => {
    const expected = formatExpected(getAtPath(config, spec.path));
    if (expected === "") {
      return {
        label: spec.label,
        read: spec.read,
        expected: "(not configured)",
        actual: normalizeOnChain(trySimulate(ctx, spec.read), spec.kind),
        passed: true,
      };
    }
    const actual = normalizeOnChain(trySimulate(ctx, spec.read), spec.kind);
    return {
      label: spec.label,
      read: spec.read,
      expected,
      actual,
      passed: actual === expected,
    };
  });
}

// ───────────────────────────────── commands ─────────────────────────────────

function cmdStatus(ctx: CliContext): number {
  const state = trySimulate(ctx, "vault_state");
  const numbers = state ? parseNumbers(state) : [];
  const totalShares = numbers[0] ?? "0";
  const totalDeposited = numbers[1] ?? "0";
  const rateBps = trySimulate(ctx, "get_reward_rate_bps");
  const rewardPool = trySimulate(ctx, "get_reward_pool_balance");
  const paused = trySimulate(ctx, "is_paused");
  const insolvent = trySimulate(ctx, "is_pool_insolvent");
  const depositors = trySimulate(ctx, "get_depositor_count");

  const report = {
    contractId: ctx.contractId,
    network: ctx.rpcUrl ?? ctx.network,
    totalShares,
    totalDeposited,
    totalDepositedDisplay: formatUnits(totalDeposited, ctx.decimals),
    rewardRateBps: rateBps ? parseLastNumber(rateBps) : null,
    rewardPool: rewardPool ? parseLastNumber(rewardPool) : null,
    rewardPoolDisplay: rewardPool ? formatUnits(parseLastNumber(rewardPool), ctx.decimals) : null,
    paused: paused ? parseBool(paused) : null,
    insolvent: insolvent ? parseBool(insolvent) : null,
    depositorCount: depositors ? Number(parseLastNumber(depositors)) : null,
  };

  const rows: Array<[string, string]> = [
    ["Contract", ctx.contractId],
    ["Network", report.network],
    ["Total shares", report.totalShares],
    ["Total deposited", `${report.totalDepositedDisplay} (${report.totalDeposited} raw)`],
    ["Reward rate", report.rewardRateBps === null ? "(unavailable)" : `${report.rewardRateBps} bps`],
    ["Reward pool", report.rewardPoolDisplay === null ? "(unavailable)" : report.rewardPoolDisplay],
    ["Paused", report.paused === null ? "(unavailable)" : String(report.paused)],
    ["Insolvent", report.insolvent === null ? "(unavailable)" : String(report.insolvent)],
    ["Depositors", report.depositorCount === null ? "(unavailable)" : String(report.depositorCount)],
  ];
  emit(ctx, report, "Vault status", rows);
  return 0;
}

function cmdSolvency(ctx: CliContext): number {
  const insolventRaw = trySimulate(ctx, "is_pool_insolvent");
  const state = trySimulate(ctx, "vault_state");
  const numbers = state ? parseNumbers(state) : [];
  const totalShares = numbers[0] ?? "0";
  const totalDeposited = numbers[1] ?? "0";
  const insuranceFund = trySimulate(ctx, "get_insurance_fund_balance");

  const insolvent = insolventRaw ? parseBool(insolventRaw) : null;
  const report = {
    contractId: ctx.contractId,
    insolvent,
    solvent: insolvent === null ? null : !insolvent,
    totalShares,
    totalDeposited,
    totalDepositedDisplay: formatUnits(totalDeposited, ctx.decimals),
    insuranceFund: insuranceFund ? parseLastNumber(insuranceFund) : null,
    insuranceFundDisplay: insuranceFund
      ? formatUnits(parseLastNumber(insuranceFund), ctx.decimals)
      : null,
  };

  const rows: Array<[string, string]> = [
    ["Insolvent", insolvent === null ? "(unavailable)" : String(insolvent)],
    ["Solvent", report.solvent === null ? "(unavailable)" : String(report.solvent)],
    ["Total shares", report.totalShares],
    ["Total deposited", `${report.totalDepositedDisplay} (${report.totalDeposited} raw)`],
    [
      "Insurance fund",
      report.insuranceFundDisplay === null ? "(unavailable)" : report.insuranceFundDisplay,
    ],
  ];
  emit(ctx, report, "Vault solvency", rows);
  return 0;
}

function cmdConfig(ctx: CliContext): number {
  const admin = trySimulate(ctx, "get_admin");
  const rateBps = trySimulate(ctx, "get_reward_rate_bps");
  const perfFee = trySimulate(ctx, "get_performance_fee_bps");
  const insuranceRate = trySimulate(ctx, "get_insurance_rate_bps");
  const rewardPool = trySimulate(ctx, "get_reward_pool_balance");
  const maxDepositCap = trySimulate(ctx, "get_max_deposit_cap");
  const paused = trySimulate(ctx, "is_paused");
  const boost = trySimulate(ctx, "get_boost_schedule");

  const num = (raw: string | null): string | null => {
    if (!raw) return null;
    try {
      return parseLastNumber(raw);
    } catch {
      return null;
    }
  };

  const report = {
    contractId: ctx.contractId,
    admin: admin ? parseAddress(admin) ?? admin : null,
    rewardRateBps: num(rateBps),
    performanceFeeBps: num(perfFee),
    insuranceRateBps: num(insuranceRate),
    rewardPool: num(rewardPool),
    maxDepositCap: num(maxDepositCap),
    paused: paused ? parseBool(paused) : null,
    boostSchedule: boost,
  };

  const show = (v: string | null): string => (v === null ? "(unavailable)" : v);
  const rows: Array<[string, string]> = [
    ["Contract", ctx.contractId],
    ["Admin", show(report.admin)],
    ["Reward rate", report.rewardRateBps === null ? "(unavailable)" : `${report.rewardRateBps} bps`],
    [
      "Performance fee",
      report.performanceFeeBps === null ? "(unavailable)" : `${report.performanceFeeBps} bps`,
    ],
    [
      "Insurance rate",
      report.insuranceRateBps === null ? "(unavailable)" : `${report.insuranceRateBps} bps`,
    ],
    ["Max deposit cap", show(report.maxDepositCap)],
    ["Reward pool", show(report.rewardPool)],
    ["Paused", report.paused === null ? "(unavailable)" : String(report.paused)],
  ];
  emit(ctx, report, "Vault configuration", rows);
  return 0;
}

function cmdParamsDiff(ctx: CliContext, configPath: string): number {
  if (!fs.existsSync(configPath)) {
    console.error(`Config file not found: ${configPath}`);
    return 2;
  }
  const config = parseToml(fs.readFileSync(configPath, "utf-8"));
  const comparisons = collectParamComparisons(ctx, config);
  const mismatches = comparisons.filter((c) => !c.passed).length;

  const rows: Array<[string, string]> = comparisons.map((c) => [
    c.label,
    `${c.passed ? "match" : "differs"} (config=${c.expected}, chain=${c.actual} via ${c.read})`,
  ]);
  emit(
    ctx,
    { contractId: ctx.contractId, configPath, mismatches, comparisons },
    `Parameter diff vs ${configPath}`,
    rows,
  );

  if (ctx.format === "table") {
    console.log("");
    console.log(
      mismatches === 0
        ? "All compared parameters match."
        : `${mismatches} parameter(s) differ from the config file.`,
    );
  }
  return mismatches === 0 ? 0 : 1;
}

function cmdSetRate(ctx: CliContext, bps: string): number {
  if (bps === "" || isNaN(Number(bps))) {
    console.error("set-rate requires a numeric basis-point value, e.g. `set-rate 500`.");
    return 2;
  }
  const result = submit(ctx, "set_reward_rate_bps", ["--rate_bps", bps]);
  emit(ctx, { action: "set_rate", rateBps: Number(bps), ...result }, "set-rate", [
    ["Reward rate", `${bps} bps`],
    ["Simulated", result.simulated || "(no output)"],
    ["Submitted", result.submitted ? "yes" : "no (dry-run)"],
  ]);
  return 0;
}

function cmdFund(ctx: CliContext, amount: string, from?: string): number {
  if (amount === "" || isNaN(Number(amount))) {
    console.error("fund requires a numeric amount in base units, e.g. `fund 25000000`.");
    return 2;
  }
  const admin = from ?? resolveIdentityAddress(ctx.identity);
  const result = submit(ctx, "fund_reward_pool", ["--admin_addr", admin, "--amount", amount]);
  emit(ctx, { action: "fund", admin, amount, ...result }, "fund", [
    ["From", admin],
    ["Amount", `${formatUnits(amount, ctx.decimals)} (${amount} raw)`],
    ["Simulated", result.simulated || "(no output)"],
    ["Submitted", result.submitted ? "yes" : "no (dry-run)"],
  ]);
  return 0;
}

function cmdPause(ctx: CliContext, reason: string, message: string): number {
  const result = submit(ctx, "pause", ["--reason", reason, "--message", message]);
  emit(ctx, { action: "pause", reason, message, ...result }, "pause", [
    ["Reason", reason],
    ["Message", message],
    ["Simulated", result.simulated || "(no output)"],
    ["Submitted", result.submitted ? "yes" : "no (dry-run)"],
  ]);
  return 0;
}

function cmdUnpause(ctx: CliContext): number {
  const result = submit(ctx, "unpause", []);
  emit(ctx, { action: "unpause", ...result }, "unpause", [
    ["Simulated", result.simulated || "(no output)"],
    ["Submitted", result.submitted ? "yes" : "no (dry-run)"],
  ]);
  return 0;
}

// ────────────────────────────── argument parsing ────────────────────────────

interface ParsedArgs {
  command: string;
  positionals: string[];
  contractId: string;
  identity: string;
  network: string;
  rpcUrl?: string;
  networkPassphrase?: string;
  decimals: number;
  dryRun: boolean;
  iUnderstand: boolean;
  format: OutputFormat;
  configPath: string;
  from?: string;
  reason: string;
  message: string;
  help: boolean;
}

function splitInline(arg: string): [string, string | undefined] {
  const eq = arg.indexOf("=");
  if (eq === -1) return [arg, undefined];
  return [arg.slice(0, eq), arg.slice(eq + 1)];
}

function parseArgs(argv: string[]): ParsedArgs {
  const args: ParsedArgs = {
    command: "",
    positionals: [],
    contractId: process.env.CONTRACT_ID ?? "",
    identity: process.env.IDENTITY ?? "",
    network: process.env.NETWORK ?? "testnet",
    rpcUrl: process.env.RPC_URL ?? process.env.STELLAR_RPC_URL,
    networkPassphrase: process.env.NETWORK_PASSPHRASE ?? process.env.STELLAR_NETWORK_PASSPHRASE,
    decimals: process.env.DECIMALS ? Number(process.env.DECIMALS) : 7,
    dryRun: false,
    iUnderstand: false,
    format: process.env.OUTPUT_FORMAT === "json" ? "json" : "table",
    configPath: process.env.CONFIG_PATH ?? "config/testnet.toml",
    reason: "Maintenance",
    message: "",
    help: false,
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const [flag, inline] = splitInline(arg);
    if (!flag.startsWith("-")) {
      if (args.command === "") {
        args.command = flag;
      } else {
        args.positionals.push(flag);
      }
      continue;
    }
    const need = (): string => {
      if (inline !== undefined) return inline;
      const value = argv[++i];
      if (value === undefined) {
        throw new Error(`Missing value for ${flag}`);
      }
      return value;
    };

    switch (flag) {
      case "-h":
      case "--help":
        args.help = true;
        break;
      case "--contract-id":
        args.contractId = need();
        break;
      case "--identity":
      case "--source":
        args.identity = need();
        break;
      case "--network":
        args.network = need();
        break;
      case "--rpc-url":
        args.rpcUrl = need();
        break;
      case "--network-passphrase":
        args.networkPassphrase = need();
        break;
      case "--decimals":
        args.decimals = Number(need());
        break;
      case "--config":
        args.configPath = need();
        break;
      case "--from":
      case "--admin":
        args.from = need();
        break;
      case "--reason":
        args.reason = need();
        break;
      case "--message":
        args.message = need();
        break;
      case "--format":
        args.format = need() === "json" ? "json" : "table";
        break;
      case "--json":
        args.format = "json";
        break;
      case "--dry-run":
        args.dryRun = true;
        break;
      case "--i-understand":
        args.iUnderstand = true;
        break;
      default:
        throw new Error(`Unknown argument: ${flag}`);
    }
  }

  return args;
}

function toContext(args: ParsedArgs): CliContext {
  return {
    contractId: args.contractId,
    identity: args.identity,
    network: args.network,
    rpcUrl: args.rpcUrl,
    networkPassphrase: args.networkPassphrase,
    decimals: args.decimals,
    dryRun: args.dryRun,
    iUnderstand: args.iUnderstand,
    format: args.format,
  };
}

function printHelp(): void {
  console.log(`vaultctl — operator CLI for the Stellar DeFi Vault

Usage:
  npx tsx scripts/vaultctl.ts [options] <command> [args]

Commands:
  status                     Pool health snapshot
  solvency                   Insolvency flag, totals, insurance fund
  config                     Dump live on-chain configuration
  params diff                Diff config TOML against on-chain parameters
  set-rate <bps>             Admin: set the reward rate (basis points)
  fund <amount>              Admin: top up the reward pool (raw base units)
  pause [reason] [message]   Admin: pause the pool
  unpause                    Admin: resume the pool

Options (or the matching environment variable):
  --contract-id <id>          $CONTRACT_ID
  --identity <name>           $IDENTITY (used as --source)
  --network <name>            $NETWORK (default: testnet)
  --rpc-url <url>             $RPC_URL / $STELLAR_RPC_URL
  --network-passphrase <p>    $NETWORK_PASSPHRASE / $STELLAR_NETWORK_PASSPHRASE
  --decimals <n>              $DECIMALS (default: 7)
  --config <path>             $CONFIG_PATH (default: config/testnet.toml)
  --from <address>            Admin address for fund (default: identity)
  --reason <name>             Pause reason (default: Maintenance)
  --message <text>            Pause message (default: empty)
  --dry-run                   Simulate writes but never submit
  --i-understand              Required to submit a mainnet write
  --json                      Output JSON instead of a table (--format json)
  -h, --help                  Show this help

Exit codes: 0 success, 1 a parameter diff mismatch, 2 usage/runtime error.
`);
}

const WRITE_COMMANDS = new Set(["set-rate", "fund", "pause", "unpause"]);

function run(args: ParsedArgs): number {
  if (args.help) {
    printHelp();
    return 0;
  }
  if (args.command === "") {
    printHelp();
    return 2;
  }
  if (!args.contractId) {
    console.error("Error: --contract-id (or $CONTRACT_ID) is required.");
    return 2;
  }
  if (!args.identity) {
    console.error("Error: --identity (or $IDENTITY) is required.");
    return 2;
  }

  const ctx = toContext(args);
  if (WRITE_COMMANDS.has(args.command) && ctx.dryRun) {
    console.error("Dry-run: no transaction will be submitted.");
  }

  switch (args.command) {
    case "status":
      return cmdStatus(ctx);
    case "solvency":
      return cmdSolvency(ctx);
    case "config":
      return cmdConfig(ctx);
    case "params": {
      const sub = args.positionals[0];
      if (sub !== "diff") {
        console.error("Usage: vaultctl params diff [--config <path>]");
        return 2;
      }
      return cmdParamsDiff(ctx, args.configPath);
    }
    case "set-rate":
      return cmdSetRate(ctx, args.positionals[0] ?? "");
    case "fund":
      return cmdFund(ctx, args.positionals[0] ?? "", args.from);
    case "pause":
      return cmdPause(ctx, args.positionals[0] ?? args.reason, args.positionals[1] ?? args.message);
    case "unpause":
      return cmdUnpause(ctx);
    default:
      console.error(`Unknown command: ${args.command}`);
      printHelp();
      return 2;
  }
}

function main(): void {
  let args: ParsedArgs;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (err) {
    console.error(`Error: ${(err as Error).message}`);
    printHelp();
    process.exitCode = 2;
    return;
  }

  try {
    process.exitCode = run(args);
  } catch (err) {
    console.error(`Error: ${(err as Error).message}`);
    process.exitCode = 2;
  }
}

if (require.main === module) {
  main();
}
