import * as anchor from "@coral-xyz/anchor";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  createMint,
  createAssociatedTokenAccount,
  mintTo,
  getAssociatedTokenAddress,
  getAccount,
} from "@solana/spl-token";

export const USDC_DECIMALS = 6;
export const CONFIG_SEED = Buffer.from("config");
export const TASK_SEED = Buffer.from("task");
export const CLAIM_SEED = Buffer.from("claim");

export function randomId16(): Buffer {
  const buf = Buffer.alloc(16);
  for (let i = 0; i < 16; i++) buf[i] = Math.floor(Math.random() * 256);
  return buf;
}

export function configPda(programId: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync([CONFIG_SEED], programId);
}

export function taskPda(programId: PublicKey, taskId: Buffer): [PublicKey, number] {
  return PublicKey.findProgramAddressSync([TASK_SEED, taskId], programId);
}

export function claimPda(programId: PublicKey, claimId: Buffer): [PublicKey, number] {
  return PublicKey.findProgramAddressSync([CLAIM_SEED, claimId], programId);
}

/** Fund `keypair` with `sol` lamports from `provider.connection`. */
export async function airdrop(
  provider: anchor.AnchorProvider,
  to: PublicKey,
  sol = 10
): Promise<void> {
  const sig = await provider.connection.requestAirdrop(to, sol * LAMPORTS_PER_SOL);
  await provider.connection.confirmTransaction(sig, "confirmed");
}

export async function makeFundedKeypair(
  provider: anchor.AnchorProvider,
  sol = 10
): Promise<Keypair> {
  const kp = Keypair.generate();
  await airdrop(provider, kp.publicKey, sol);
  return kp;
}

/** Create a fresh mint (fake USDC, 6 decimals). */
export async function createFakeUsdcMint(
  provider: anchor.AnchorProvider,
  authority: Keypair
): Promise<PublicKey> {
  return createMint(
    provider.connection,
    authority,
    authority.publicKey,
    null,
    USDC_DECIMALS
  );
}

/** Create an ATA (if needed) and mint `amountMicro` tokens into it. */
export async function ensureAtaWithBalance(
  provider: anchor.AnchorProvider,
  mint: PublicKey,
  owner: PublicKey,
  payer: Keypair,
  mintAuthority: Keypair,
  amountMicro: bigint
): Promise<PublicKey> {
  const ata = await getAssociatedTokenAddress(mint, owner);
  try {
    await getAccount(provider.connection, ata);
  } catch {
    await createAssociatedTokenAccount(provider.connection, payer, mint, owner);
  }
  if (amountMicro > 0n) {
    await mintTo(
      provider.connection,
      payer,
      mint,
      ata,
      mintAuthority,
      Number(amountMicro)
    );
  }
  return ata;
}

export async function tokenBalance(
  provider: anchor.AnchorProvider,
  ata: PublicKey
): Promise<bigint> {
  try {
    const acc = await getAccount(provider.connection, ata);
    return acc.amount;
  } catch {
    return 0n;
  }
}

export function usdc(amount: number): bigint {
  return BigInt(Math.round(amount * 10 ** USDC_DECIMALS));
}

/** Convenience: run `fn` and expect it to throw with a message that includes `needle`. */
export async function expectError(
  fn: () => Promise<unknown>,
  needle: string
): Promise<void> {
  let caught: unknown = null;
  try {
    await fn();
  } catch (e) {
    caught = e;
  }
  if (!caught) {
    throw new Error(`Expected error containing "${needle}", got success`);
  }
  const msg = (caught as Error).message ?? String(caught);
  if (!msg.includes(needle)) {
    throw new Error(`Expected error to include "${needle}", got: ${msg}`);
  }
}

export const BN = anchor.BN;
export type BNType = anchor.BN;
export { PublicKey, Keypair, SystemProgram, SYSVAR_RENT_PUBKEY };
export { TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID, getAssociatedTokenAddress };
