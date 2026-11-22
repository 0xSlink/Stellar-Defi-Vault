#!/usr/bin/env tsx
/**
 * Example script: wrap a user-signed `deposit` transaction in a sponsor-signed
 * Stellar fee-bump transaction so the user pays 0 XLM in fees.
 *
 * Usage:
 *   CONTRACT_ID=C... USER_SECRET=S... SPONSOR_SECRET=S... AMOUNT=10000000 \
 *     npx tsx scripts/fee-bump-deposit.ts
 */

import {
  Contract,
  FeeBumpTransaction,
  Keypair,
  Networks,
  Transaction,
  TransactionBuilder,
  nativeToScVal,
  rpc,
} from "@stellar/stellar-sdk";

function requiredEnv(name: string): string {
  const val = process.env[name]?.trim();
  if (!val) {
    throw new Error(`${name} environment variable is required`);
  }
  return val;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function buildUserSignedDeposit(
  server: rpc.Server,
  contractId: string,
  user: Keypair,
  amount: bigint,
  networkPassphrase: string,
): Promise<Transaction> {
  const userAccount = await server.getAccount(user.publicKey());
  const contract = new Contract(contractId);

  const rawTx = new TransactionBuilder(userAccount, {
    fee: "100",
    networkPassphrase,
  })
    .addOperation(
      contract.call(
        "deposit",
        nativeToScVal(user.publicKey(), { type: "address" }),
        nativeToScVal(amount, { type: "i128" }),
      ),
    )
    .setTimeout(60)
    .build();

  const prepared = await server.prepareTransaction(rawTx);
  prepared.sign(user);
  return prepared;
}

function wrapInFeeBump(
  innerTx: Transaction,
  sponsor: Keypair,
  maxFeeStroops: string,
  networkPassphrase: string,
): FeeBumpTransaction {
  const innerFee = BigInt(innerTx.fee);
  const cap = BigInt(maxFeeStroops);
  if (innerFee > cap) {
    throw new Error(
      `Inner tx fee (${innerFee}) exceeds sponsor maxFee cap (${cap})`,
    );
  }

  const feeBump = TransactionBuilder.buildFeeBumpTransaction(
    sponsor,
    maxFeeStroops,
    innerTx,
    networkPassphrase,
  );
  feeBump.sign(sponsor);
  return feeBump;
}

async function main(): Promise<void> {
  const contractId = requiredEnv("CONTRACT_ID");
  const user = Keypair.fromSecret(requiredEnv("USER_SECRET"));
  const sponsor = Keypair.fromSecret(requiredEnv("SPONSOR_SECRET"));
  const amount = BigInt(process.env.AMOUNT ?? "10000000");
  const maxFeeStroops = process.env.MAX_SPONSOR_FEE ?? "500000";
  const rpcUrl =
    process.env.RPC_URL?.trim() ?? "https://soroban-testnet.stellar.org";
  const networkPassphrase =
    process.env.NETWORK_PASSPHRASE?.trim() ?? Networks.TESTNET;

  const server = new rpc.Server(rpcUrl);

  console.log(`Building user-signed deposit (${amount} stroops) for ${user.publicKey()}...`);
  const innerTx = await buildUserSignedDeposit(
    server,
    contractId,
    user,
    amount,
    networkPassphrase,
  );

  console.log(`Wrapping inner tx in fee-bump sponsored by ${sponsor.publicKey()} (maxFee=${maxFeeStroops})...`);
  const feeBumpTx = wrapInFeeBump(
    innerTx,
    sponsor,
    maxFeeStroops,
    networkPassphrase,
  );

  const sent = await server.sendTransaction(feeBumpTx);
  if (sent.status === "ERROR") {
    throw new Error(`sendTransaction failed: ${JSON.stringify(sent.errorResult)}`);
  }

  console.log(`Submitted fee-bump tx ${sent.hash}; waiting for confirmation...`);
  for (let i = 0; i < 20; i++) {
    const res = await server.getTransaction(sent.hash);
    if (res.status === "SUCCESS") {
      console.log(`Confirmed in ledger ${res.ledger} (hash: ${sent.hash})`);
      return;
    }
    if (res.status === "FAILED") {
      throw new Error(`Fee-bump transaction ${sent.hash} FAILED`);
    }
    await sleep(1_500);
  }
  throw new Error(`Timed out waiting for transaction ${sent.hash}`);
}

void main();
