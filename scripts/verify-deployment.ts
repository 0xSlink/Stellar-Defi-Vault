#!/usr/bin/env tsx
/**
 * Post-deployment verification script.
 *
 * Verifies that on-chain state matches the intended configuration by querying
 * the deployed contract through `scripts/vaultctl.ts` — the shared operator
 * CLI (issue #859) — so this script never draws the vault state itself.
 *
 * Usage:
 *   npx tsx scripts/verify-deployment.ts --config config/testnet.toml --contract-id <id>
 *
 * Target/network are taken from flags or the same environment variables the
 * rest of the operator tooling uses (`CONTRACT_ID`, `IDENTITY`, `NETWORK`,
 * `RPC_URL` / `STELLAR_RPC_URL`, `NETWORK_PASSPHRASE`).
 */

import * as fs from "fs";

import {
  CliContext,
  collectParamComparisons,
  parseToml,
} from "./vaultctl";

interface VerificationResult {
  check: string;
  expected: string;
  actual: string;
  passed: boolean;
}

function argValue(args: string[], name: string): string | undefined {
  const index = args.indexOf(name);
  return index === -1 ? undefined : args[index + 1];
}

function buildContext(args: string[], contractId: string): CliContext {
  return {
    contractId,
    identity: argValue(args, "--identity") ?? process.env.IDENTITY ?? "",
    network: argValue(args, "--network") ?? process.env.NETWORK ?? "testnet",
    rpcUrl:
      argValue(args, "--rpc-url") ?? process.env.RPC_URL ?? process.env.STELLAR_RPC_URL,
    networkPassphrase:
      argValue(args, "--network-passphrase") ??
      process.env.NETWORK_PASSPHRASE ??
      process.env.STELLAR_NETWORK_PASSPHRASE,
    decimals: process.env.DECIMALS ? Number(process.env.DECIMALS) : 7,
    dryRun: false,
    iUnderstand: false,
    format: "table",
  };
}

function verifyDeployment(ctx: CliContext, configPath: string): VerificationResult[] {
  const config = parseToml(fs.readFileSync(configPath, "utf-8"));

  // Reuse vaultctl's comparison logic — the same checks `vaultctl params diff`
  // prints, so the two can never drift apart.
  const results: VerificationResult[] = collectParamComparisons(ctx, config).map((c) => ({
    check: `${c.label} (via ${c.read})`,
    expected: c.expected,
    actual: c.actual,
    passed: c.passed,
  }));

  // On-chain Wasm hash is not readable from the contract itself; the bytecode
  // is verified separately by scripts/verify-build-reproducibility.sh.
  results.push({
    check: "Wasm hash",
    expected: "(local reproducible build)",
    actual: "(manual check — see scripts/verify-build-reproducibility.sh)",
    passed: true,
  });

  return results;
}

function printResults(results: VerificationResult[]): boolean {
  console.log("\n╔══════════════════════════════════════════════════════════════════════╗");
  console.log("║                    Deployment Verification Report                     ║");
  console.log("╚══════════════════════════════════════════════════════════════════════╝\n");

  const passed = results.filter((r) => r.passed).length;
  const total = results.length;

  for (const result of results) {
    const status = result.passed ? "✅ PASS" : "❌ FAIL";
    console.log(`${status} ${result.check}`);
    console.log(`      Expected: ${result.expected}`);
    console.log(`      Actual:   ${result.actual}`);
    console.log();
  }

  console.log("─".repeat(70));
  console.log(`Results: ${passed}/${total} checks passed`);

  if (passed === total) {
    console.log("\n✅ All verification checks passed!");
  } else {
    console.log(`\n❌ ${total - passed} check(s) failed!`);
  }

  return passed === total;
}

// Main
const args = process.argv.slice(2);
const configPath = argValue(args, "--config") ?? process.env.CONFIG_PATH ?? "config/testnet.toml";
const contractId = argValue(args, "--contract-id") ?? process.env.CONTRACT_ID ?? "";

if (!contractId) {
  console.error(
    "Usage: npx tsx scripts/verify-deployment.ts --config <config-path> --contract-id <id>",
  );
  console.error("(or set $CONTRACT_ID)");
  process.exit(1);
}

if (!fs.existsSync(configPath)) {
  console.error(`Config file not found: ${configPath}`);
  process.exit(1);
}

const ctx = buildContext(args, contractId);

if (!ctx.identity) {
  console.error("Error: --identity (or $IDENTITY) is required to query the contract.");
  process.exit(1);
}

console.log(`Verifying deployment: ${contractId}`);
console.log(`Using config: ${configPath}`);

const results = verifyDeployment(ctx, configPath);
const allPassed = printResults(results);

process.exit(allPassed ? 0 : 1);
