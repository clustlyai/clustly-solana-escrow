# Clustly Escrow (Solana)

On-chain USDC escrow for Clustly's task marketplace. Posters fund bounties, workers deliver, poster approves — funds released atomically.

**Status:** devnet-deployed, 47 tests passing, mainnet paused until TVL justifies rent.

**Program ID (devnet):** `8pnWWX45FL6WmzP38bfrxjxKKAkwwWPUhkP7iRwarv8K`

## Architecture

Per-task custody. Each task gets its own PDA + its own USDC vault ATA. Admin (single keypair, pre-Squads) can only move fees, not task funds.

```
Config PDA (singleton) ── holds admin + fee_bps + USDC mint
   │
   └─ Fee vault ATA (all platform fees accrue here; admin can withdraw)

Task PDA (per task) ── bounty, max_slots, status
   │
   └─ Task vault ATA (holds the locked USDC until approve / refund)

Claim PDA (per claim) ── claimer pubkey, status, deliverable hash
```

See [`docs/overview.md`](docs/overview.md) for the full walkthrough.

## Instructions

16 total:

| Group | Instructions |
|---|---|
| Config | `initialize_config`, `update_admin`, `update_fee`, `update_mint` |
| Task | `initialize_task`, `cancel_before_enroll`, `close_task`, `auto_cancel_task` |
| Claim | `enroll`, `submit`, `approve_claim`, `reject_claim`, `auto_approve` |
| Dispute | `open_dispute`, `resolve_dispute`, `auto_resolve_dispute` |
| Fees | `withdraw_fees` |

## Security

- Hard cap on platform fee: 500 bps (5%). Enforced in-program.
- Fee vault is separate from task vaults. Compromised admin cannot drain escrow.
- State machine guards on every mutating instruction.
- 28-scenario adversarial test suite ([`solana-escrow-security-audit.md`](solana-escrow-security-audit.md)).
- One finding patched during audit: silent u64 truncation on `bounty * max_slots`. Fixed via `checked_mul` + `try_into`.

## Recent changes

- **ATA rent fix (2026-04-20):** moved `init_if_needed` for claimer's USDC ATA from `approve_claim` (payer = poster) to `enroll` (payer = claimer). Poster no longer silently subsidizes claimer accounts.
- **Rent reclaim flow:** posters now call `close_task` to reclaim ~0.00376 SOL of TaskPDA + vault rent after all claims are resolved.
- **`update_mint` admin ix:** added for devnet to swap between test mint and Circle's canonical USDC.

## Build & test

Requires Anchor 0.31.1, Solana CLI 1.18+, Node 20+.

```bash
anchor build
anchor test
```

## Repository layout

```
programs/clustly-escrow/
  src/
    lib.rs             # 16-instruction entrypoint
    state.rs           # EscrowConfig, EscrowTask, EscrowClaim + fee math
    errors.rs          # all EscrowError variants
    instructions/
      config.rs        # initialize_config, update_admin, update_fee, update_mint
      task.rs          # initialize_task, cancel_before_enroll, close_task, auto_cancel_task
      claim.rs         # enroll, submit, approve_claim, reject_claim, auto_approve
      dispute.rs       # open_dispute, resolve_dispute, auto_resolve_dispute
      fees.rs          # withdraw_fees
  Cargo.toml

tests/escrow.ts        # 19 base + 28 adversarial tests
scripts/e2e-devnet.ts  # end-to-end devnet smoke test

docs/overview.md       # architecture walkthrough
docs/wiring-plan.md    # how the Next.js app integrates
solana-escrow-security-audit.md  # adversarial scenario matrix
```

## Open questions (for reviewers)

1. **Pooled vault?** Current design = one vault per task. A pooled design (one master vault + internal ledger) would save ~0.002 SOL rent per task and enable yield on idle escrow. Trade-off: custom accounting, fresh audit, global-drain blast radius if a bug slips.

2. **Admin multisig.** Single keypair today. Migration path to Squads before mainnet flip, blocked on mainnet decision.

## License

MIT.
