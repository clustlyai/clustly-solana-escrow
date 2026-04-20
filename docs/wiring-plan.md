# Wiring Plan — Cutover from Hot Wallet to Program

This document describes the **followup PR** that replaces the hot-wallet
Solana escrow with calls to the deployed program. The program PR (this
branch) intentionally does NOT do any of the wiring below. Shipping that
separately keeps the program's PR small and easy to audit.

## What this PR will change

- `src/lib/tasks/escrow-solana.ts` — replace hot-wallet send calls with
  program instructions.
- `src/lib/blockchain/wallet-solana.ts` — marked deprecated; eventually
  deleted after in-flight tasks drain.
- `src/lib/blockchain/verify-solana.ts` — also deprecated; the program
  handles verification on-chain, so we no longer need an RPC re-read path.
- `src/app/api/tasks/[id]/deposit/route.ts` — no longer needs on-chain
  verification; just records the task PDA + signature after the frontend
  confirms the `initialize_task` tx.
- `src/app/api/tasks/[id]/claims/[claimId]/approve/route.ts` — backend no
  longer signs anything. The poster signs `approve_claim` in their wallet;
  the API route just records the result.
- DB migration: add `tasks.escrow_program_account TEXT`,
  `task_claims.escrow_claim_account TEXT`. Store the PDA addresses so the
  app layer can look up on-chain state.
- Environment: keep `SOLANA_HOT_WALLET_PRIVATE_KEY` during the transition
  (still serves legacy tasks), but stop USING it for new tasks. Delete
  after cutover.
- Add an `ADMIN_KEYPAIR` env var (or Squads multisig integration) — needed
  for `resolve_dispute` on the admin side.

## What this PR will NOT change

- Task-creation UX (title, description, category, tags). Still Supabase.
- Tipping. Frozen per root `CLAUDE.md`.
- Base escrow. Separate PR later.

## Steps, in order

### 1. Add the Anchor client

```bash
cd /Users/kang5647/dev/0xmedia/clustly-web
npm install @coral-xyz/anchor
```

### 2. Copy the IDL into Next.js

```
cp solana/target/idl/clustly_escrow.json src/lib/solana/idl/clustly_escrow.json
```

Add the program ID as a constant (NOT to `src/lib/constants.ts` — that file is
frozen for tipping). Use `src/lib/solana/program.ts` instead:

```ts
export const CLUSTLY_ESCROW_PROGRAM_ID = new PublicKey(
  "8pnWWX45FL6WmzP38bfrxjxKKAkwwWPUhkP7iRwarv8K" // devnet
);
```

### 3. Write a thin client wrapper

`src/lib/solana/program.ts`:

```ts
export function getEscrowProgram(provider: AnchorProvider): Program<ClustlyEscrow> { … }
export function taskPda(taskId: Uint8Array): PublicKey { … }
export function claimPda(claimId: Uint8Array): PublicKey { … }
export function configPda(): PublicKey { … }
export function taskIdFromUuid(uuid: string): Uint8Array { … }
```

Keep it focused: derivations and instruction builders. No state, no caching.

### 4. Refactor `src/lib/tasks/escrow-solana.ts`

Each existing function maps to a program call:

| Legacy function | Replace with |
|---|---|
| `verifySolanaEscrowDeposit` | Delete. Deposit verification is on-chain now — the tx itself either succeeded (task exists) or didn't. |
| `recordSolanaEscrowDeposit` | Keep, but just writes `escrow_program_account`, `escrow_tx_signature`, `payment_status='deposited'` to DB after the frontend confirms the tx. |
| `releaseSolanaEscrow` | The poster signs `approve_claim` in-browser. Backend records the result. |
| `releaseSolanaClaimEscrow` | Same as above for multi-slot. |
| `refundSolanaEscrow` / `closeSolanaTaskAndRefund` | Map to `cancel_before_enroll` / `close_task`. Poster signs from frontend. |

### 5. Update API routes

- `POST /api/tasks/[id]/deposit` — accept a tx signature, verify the tx
  confirmed, read the program account, update DB. No on-chain sends from the
  backend.
- `POST /api/tasks/[id]/claims/[claimId]/approve` — same pattern: frontend
  signs, backend just records.
- `POST /api/admin/disputes/claims/[claimId]/resolve` — backend signs with
  `ADMIN_KEYPAIR` (eventually Squads multisig).

### 6. Frontend signing flows

Pages that need wallet signatures:

- Task creation page → after DB insert, poster signs `initialize_task`
- Enroll button → claimer signs `enroll`
- Submit deliverable → claimer signs `submit` with the content hash
- Approve button (poster's task detail view) → poster signs `approve_claim`
- Reject button → poster signs `reject_claim`
- Dispute button (deliverer's claim detail) → claimer signs `open_dispute`

Use the existing `@solana/wallet-adapter-react` setup. Hook pattern:

```ts
const { connection } = useConnection();
const wallet = useWallet();
const program = useMemo(
  () => getEscrowProgram(new AnchorProvider(connection, wallet, {})),
  [connection, wallet]
);
```

### 7. Admin operations

Admin runs a separate Node process (cron or manual trigger) with its own
keypair:

```ts
// scripts/solana/resolve-dispute.ts
const admin = Keypair.fromSecretKey(JSON.parse(process.env.ADMIN_KEYPAIR!));
await program.methods
  .resolveDispute({ deliverer: {} })
  .accountsStrict({ admin: admin.publicKey, … })
  .signers([admin])
  .rpc();
```

For mainnet, replace this with a Squads multisig flow. The contract already
supports rotation via `update_admin` — no code change needed.

### 8. Crank jobs

The existing hourly cron at `/api/cron/task-expiry` can call the
permissionless crank instructions (`auto_approve`, `auto_cancel_task`,
`auto_resolve_dispute`). The crank caller is anyone with SOL, so the cron's
own keypair works.

### 9. Migration strategy (in-flight tasks)

Do NOT try to migrate existing hot-wallet tasks into the program. Just let
them drain:

1. After this PR ships, new tasks use the program.
2. Existing tasks continue on the legacy hot-wallet path until naturally
   closed (approve, cancel, or 14d auto-cancel).
3. After 30 days (or all legacy tasks closed, whichever first), delete the
   hot-wallet code and env var.

Gate the routing by the task's `payment_status` + presence of
`escrow_program_account`:

```ts
if (task.escrow_program_account) {
  // route through program
} else {
  // legacy hot wallet path
}
```

### 10. Decommission checklist (after drain)

- Delete `src/lib/blockchain/wallet-solana.ts`
- Delete `src/lib/blockchain/verify-solana.ts`
- Delete the `SOLANA_HOT_WALLET_PRIVATE_KEY` env var from Vercel / .env.local
- Sweep any remaining SOL from the hot wallet to company treasury
- Remove the "legacy path" branch from `escrow-solana.ts`

## Mainnet deploy checklist (blocker for this PR shipping to prod)

Do these BEFORE pointing the frontend at mainnet:

1. Deploy the program to mainnet with a **Squads multisig** as upgrade
   authority (not a single keypair).
2. Call `initialize_config` with:
   - admin = Squads vault pubkey (single-key OK for v1, but Squads strongly
     recommended)
   - usdc_mint = `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`
   - fee_bps = 400
3. Run the e2e smoke test against mainnet with small amounts ($0.10 bounty)
   to confirm the deployed code matches the audited code.
4. Update `CLUSTLY_ESCROW_PROGRAM_ID` in the client to the mainnet ID.
5. Dry-run 3 full task lifecycles on mainnet with the team before opening
   to public traffic.

## Verification for the wiring PR

Before merging:

- `npm run build` passes
- `npm run lint` passes
- `npm test` passes (existing vitest suite)
- Existing Anchor tests still pass (`cd solana && anchor test`)
- New e2e test: create task in UI → verify TaskPDA exists on devnet → enroll
  → submit → approve → verify claimer's USDC ATA got paid. Screenshot the
  Solscan links.
- Manual QA: one full task lifecycle + one dispute resolution in dev.

## Out of scope for the wiring PR

- Performance optimization of the client (batching, caching IDL)
- Server-side decoding of program accounts for the feed (can read via RPC
  in v1, cache later)
- Token-2022 support
- Multi-mint support
- Squads multisig integration for admin (separate PR, enabled by
  `update_admin` existing already)
