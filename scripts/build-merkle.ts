/**
 * Build a Merkle root and per-leaf proofs from a CSV (issues #872 and #873).
 *
 * The on-chain verifier folds a proof with the *same* leaf and node hashing
 * used here, so a root produced by this script always verifies in
 * `deposit_with_proof` (allowlist mode) or `claim_distribution`
 * (distribution mode). See the module docs in `src/allowlist_merkle.rs` and
 * `src/merkle_distributor.rs` for the on-chain rules this mirrors:
 *
 * # Leaf hashing (domain separated)
 *
 *   allowlist    : SHA-256("stellar-defi-vault:allowlist:v1:"    || address_strkey)
 *   distribution : SHA-256("stellar-defi-vault:distribution:v1:" || address_strkey
 *                          || amount as 16-byte big-endian i128)
 *
 * # Node hashing
 *
 *   node = SHA-256(concat(min(a, b), max(a, b))) where "min"/"max" compare the
 *   first byte of the two 32-byte children. Sorting each pair makes the proof
 *   order independent, so the off-chain tree matches the on-chain fold.
 *
 * # Input
 *
 * A CSV with a header row.
 *   - allowlist:    an `address` column (extra columns are ignored).
 *   - distribution: an `address` column and an `amount` column.
 * Blank lines and `#` comments are ignored.
 *
 * # Usage
 *
 *   npx tsx scripts/build-merkle.ts --input recipients.csv --mode distribution
 *   npx tsx scripts/build-merkle.ts --input allow.csv --mode allowlist --out root.json
 *
 * # Output
 *
 * JSON on stdout (and to `--out`, if given):
 *   {
 *     "mode": "distribution",
 *     "root": "a1b2...",
 *     "entries": [
 *       { "address": "G...", "amount": "100", "leaf": "...", "proof": ["...", ...] }
 *     ]
 *   }
 *
 * `root` and every proof element are lower-case hex (no `0x`), the form the
 * Stellar CLI accepts for a `BytesN<32>` argument.
 */

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const ALLOWLIST_DOMAIN = "stellar-defi-vault:allowlist:v1:";
const DISTRIBUTION_DOMAIN = "stellar-defi-vault:distribution:v1:";

type Mode = "allowlist" | "distribution";

interface Entry {
  address: string;
  amount?: bigint;
  leaf: Buffer;
}

interface Args {
  input: string;
  mode: Mode;
  out?: string;
  help: boolean;
}

function parseArgs(argv: string[]): Args {
  const args: Args = {
    input: process.env.MERKLE_INPUT ?? "",
    mode: (process.env.MERKLE_MODE as Mode) ?? "allowlist",
    out: process.env.MERKLE_OUT,
    help: false,
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = (): string => {
      const value = argv[++i];
      if (value === undefined) {
        throw new Error(`Missing value for ${arg}`);
      }
      return value;
    };
    switch (arg) {
      case "--help":
      case "-h":
        args.help = true;
        break;
      case "--input":
        args.input = next();
        break;
      case "--mode":
        args.mode = next() as Mode;
        break;
      case "--out":
        args.out = next();
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (args.mode !== "allowlist" && args.mode !== "distribution") {
    throw new Error(`--mode must be "allowlist" or "distribution", got "${args.mode}"`);
  }
  return args;
}

function sha256(...parts: Buffer[]): Buffer {
  const hash = createHash("sha256");
  for (const part of parts) {
    hash.update(part);
  }
  return hash.digest();
}

/** 16-byte big-endian two's-complement encoding of a signed i128. */
function amountToBytes(amount: bigint): Buffer {
  const unsigned = BigInt.asUintN(128, amount);
  const bytes = Buffer.alloc(16);
  bytes.writeBigUInt64BE(unsigned >> 64n, 0);
  bytes.writeBigUInt64BE(unsigned & 0xffffffffffffffffn, 8);
  return bytes;
}

function leafHash(mode: Mode, address: string, amount?: bigint): Buffer {
  const domain = mode === "allowlist" ? ALLOWLIST_DOMAIN : DISTRIBUTION_DOMAIN;
  const parts = [Buffer.from(domain, "utf8"), Buffer.from(address, "utf8")];
  if (mode === "distribution") {
    if (amount === undefined) {
      throw new Error(`missing amount for ${address}`);
    }
    parts.push(amountToBytes(amount));
  }
  return sha256(...parts);
}

function hashPair(a: Buffer, b: Buffer): Buffer {
  const [first, second] = a[0] <= b[0] ? [a, b] : [b, a];
  return sha256(first, second);
}

function parseCsv(mode: Mode, text: string): Entry[] {
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));

  if (lines.length === 0) {
    throw new Error("input CSV is empty");
  }

  const header = lines[0].split(",").map((cell) => cell.trim().toLowerCase());
  const addressCol = header.indexOf("address");
  if (addressCol < 0) {
    throw new Error("CSV must have an 'address' column");
  }
  const amountCol = header.indexOf("amount");
  if (mode === "distribution" && amountCol < 0) {
    throw new Error("distribution mode requires an 'amount' column");
  }

  return lines.slice(1).map((line, index) => {
    const cells = line.split(",").map((cell) => cell.trim());
    const address = cells[addressCol];
    if (!address) {
      throw new Error(`row ${index + 2}: missing address`);
    }
    if (mode === "allowlist") {
      return { address, leaf: leafHash(mode, address) };
    }
    const raw = cells[amountCol];
    if (raw === undefined || raw === "") {
      throw new Error(`row ${index + 2}: missing amount for ${address}`);
    }
    let amount: bigint;
    try {
      amount = BigInt(raw);
    } catch {
      throw new Error(`row ${index + 2}: invalid amount "${raw}"`);
    }
    if (amount < 0n) {
      throw new Error(`row ${index + 2}: amount must be non-negative`);
    }
    return { address, amount, leaf: leafHash(mode, address, amount) };
  });
}

interface Tree {
  root: Buffer;
  proofs: Buffer[][];
}

/** Build levels (padding odd counts by duplicating the last node). */
function buildTree(leaves: Buffer[]): Tree {
  if (leaves.length === 0) {
    throw new Error("cannot build a tree with no leaves");
  }
  const levels: Buffer[][] = [];
  let current = leaves.slice();
  if (current.length % 2 === 1) {
    current.push(current[current.length - 1]);
  }
  levels.push(current);

  while (levels[levels.length - 1].length > 1) {
    const prev = levels[levels.length - 1];
    let next: Buffer[] = [];
    for (let i = 0; i < prev.length; i += 2) {
      next.push(hashPair(prev[i], prev[i + 1]));
    }
    if (next.length % 2 === 1 && next.length > 1) {
      next.push(next[next.length - 1]);
    }
    levels.push(next);
  }

  const proofs = leaves.map((_, index) => {
    const proof: Buffer[] = [];
    let idx = index;
    for (let level = 0; level < levels.length - 1; level++) {
      proof.push(levels[level][idx ^ 1]);
      idx >>= 1;
    }
    return proof;
  });

  return { root: levels[levels.length - 1][0], proofs };
}

function main(): void {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    process.stdout.write(
      "usage: build-merkle.ts --input <csv> [--mode allowlist|distribution] [--out <json>]\n",
    );
    return;
  }
  if (!args.input) {
    throw new Error("--input <csv> is required (or set MERKLE_INPUT)");
  }

  const entries = parseCsv(args.mode, readFileSync(args.input, "utf8"));
  const { root, proofs } = buildTree(entries.map((entry) => entry.leaf));

  const document = {
    mode: args.mode,
    root: root.toString("hex"),
    entries: entries.map((entry, index) => ({
      address: entry.address,
      ...(entry.amount !== undefined ? { amount: entry.amount.toString() } : {}),
      leaf: entry.leaf.toString("hex"),
      proof: proofs[index].map((node) => node.toString("hex")),
    })),
  };

  const json = `${JSON.stringify(document, null, 2)}\n`;
  if (args.out) {
    writeFileSync(args.out, json);
  }
  process.stdout.write(json);
}

main();
