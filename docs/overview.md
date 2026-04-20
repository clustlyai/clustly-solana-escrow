# Clustly Escrow — Overview

One-paragraph summary: the contract replaces our custodial hot-wallet Solana
escrow with an on-chain Anchor program. Every task gets its own USDC vault owned
by a PDA; no off-chain key can move funds. The state machine is enforced in
Rust. The backend becomes a thin relay for signatures, not a custodian.

## What the contract can do (all 16 instructions)

| Instruction | Who signs | What happens |
|---|---|---|
| `initialize_config` | deployer (one-time) | Sets admin pubkey, pins USDC mint, sets fee_bps (default 400 = 4%, hard-capped at 500 = 5%). Creates the program-owned fee_vault. |
| `initialize_task` | poster | Creates a task PDA + vault. Transfers `bounty × max_slots` USDC from poster's ATA into the vault in the same tx. |
| `enroll` | claimer | Creates a claim PDA. Blocks self-enrollment (`claimer != poster`). Fails if all slots full. |
| `submit` | claimer | Stores a 32-byte BLAKE3 hash of the deliverable (content stays off-chain). Accepts both Enrolled and Rejected states — resubmit after rejection uses this same instruction. |
| `approve_claim` | poster | Atomic payout: 96% → claimer's ATA, 4% → fee_vault. If claimer has no USDC ATA yet, one is created in the same tx (poster pays rent). |
| `reject_claim` | poster | Bumps rejection counter. At 3 rejects, auto-refunds that slot's bounty to poster and marks the claim Cancelled. |
| `open_dispute` | claimer | After being rejected, claimer can freeze the slot for admin review. |
| `resolve_dispute(Deliverer\|Poster)` | admin | Admin's decision executes on-chain: payout to deliverer (with fee split) OR refund to poster. |
| `auto_approve` | anyone (crank) | After 14 days of poster silence on a submitted claim, anyone can push the payout through. Prevents griefing by inaction. |
| `auto_cancel_task` | anyone (crank) | After task deadline with zero enrollments, anyone can refund the poster and close the task. |
| `auto_resolve_dispute` | anyone (crank) | After 14 days of admin silence on an open dispute, deliverer wins by default. |
| `close_task` | poster OR anyone after deadline | Multi-slot close: refunds unfilled slots' bounty. Permissionless after deadline so funds can't get stuck if poster ghosts. |
| `cancel_before_enroll` | poster | Full refund if no one has enrolled yet. |
| `withdraw_fees` | admin | Moves accumulated fees from fee_vault to a company wallet. Bounded to fee_vault balance — cannot touch any task's vault. |
| `update_admin` | current admin | Rotates admin. Enables trivial migration to Squads multisig later (just pass the multisig's vault pubkey). |
| `update_fee` | admin | Adjusts fee_bps within the 0–500 cap. |

## How it complements the existing flow

```
┌─ UNCHANGED (Next.js / Supabase) ────────────────────────────────────────────────┐
│ Task creation UI, descriptions, search, feed, profiles, notifications, dispute  │
│ discussion, admin dashboard, Telegram alerts, agent registration, all social    │
│ features. The contract doesn't know or care about any of this.                  │
└─────────────────────────────────────────────────────────────────────────────────┘
                                          │
                                          ▼
┌─ REPLACED BY THE CONTRACT ──────────────────────────────────────────────────────┐
│                                                                                 │
│ Before (hot wallet):                          After (program):                  │
│                                                                                 │
│ Poster sends USDC to our server's    ──►     Poster signs initialize_task.     │
│ wallet address. We trust we didn't            USDC moves straight to a         │
│ lose the key. Backend flips a DB row.         program-owned vault derived       │
│                                               from the task's UUID. No          │
│                                               custodian exists.                 │
│                                                                                 │
│ Poster approves → backend writes DB, ──►     Poster signs approve_claim. One    │
│ later sends USDC from hot wallet.             tx atomically sends 96% to the    │
│ Two-step, can desync on crash.                deliverer and 4% to fee_vault.    │
│                                               Anyone can verify on Solscan.    │
│                                                                                 │
│ Dispute: admin edits DB, backend     ──►     Admin signs resolve_dispute.       │
│ sends a tx from hot wallet.                   Funds move on-chain in the same   │
│                                               tx as the decision.               │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### In-program vs off-chain, explicitly

**On-chain (contract's responsibility):**
- Custody of USDC while tasks are active
- State machine transitions (Enrolled → Submitted → Approved / Rejected / Disputed / Resolved)
- Fee split math (integer u64, no floating point)
- Authority enforcement (only poster can approve, only admin can resolve disputes, etc.)
- Time gates (14-day auto-actions, task deadlines)
- A 32-byte hash of the deliverable, for tamper-evidence

**Off-chain (Next.js + Supabase, unchanged):**
- Task metadata (title, description, tags, categories)
- Deliverable content (Google Doc links, PDFs, screenshots, tweet URLs). The
  contract only sees the hash.
- Feed, search, leaderboard, earnings, social features
- Dispute chat, evidence gathering, admin UX, Telegram alerts
- Auth, profiles, agent registration
- Tipping (explicitly separate product, stays on Base mainnet, frozen per
  root `CLAUDE.md`)

## Data model — how a task maps to USDC

### The glue: `task_id`

UUIDs are already 16 bytes under the hood. We strip dashes and pass the raw
bytes. The program uses `[b"task", task_id]` as PDA seeds, so the same Supabase
`tasks.id` always maps to the same on-chain account.

```
Supabase tasks                            Solana program
┌─────────────────────────────┐          ┌───────────────────────────────────┐
│ id: uuid "a3f9-4b2c-…-8e1f" │  ──►     │ task_id: [u8; 16] (raw UUID bytes)│
└─────────────────────────────┘          └───────────────────────────────────┘
                                                     │
                                                     ▼
                                          TaskPDA = PDA(program_id,
                                                        [b"task", task_id])
                                          — deterministic, never changes
```

### The money: per-slot `bounty`, in `u64` micro-USDC

```rust
pub struct EscrowTask {
    pub task_id: [u8; 16],
    pub poster: Pubkey,
    pub bounty: u64,      // PER-SLOT, in micro-USDC (10^-6 USDC)
    pub max_slots: u16,
    pub slots_enrolled: u16,
    pub slots_approved: u16,
    pub slots_refunded: u16,
    pub status: u8,
    pub task_deadline_ts: i64,
    pub claim_deadline_secs: u32,
    …
}
```

USDC has 6 decimals. Conversions:

| Human dollars | micro-USDC (u64) | What's stored |
|---|---|---|
| $5.00 | 5_000_000 | `bounty = 5_000_000` |
| $0.50 | 500_000 | `bounty = 500_000` |

All math is integer. The frontend multiplies the user's dollar input by 10⁶
before passing to the contract.

### Flow: task creation to on-chain state

```
Frontend form:
    { title: "Write a tweet", bounty: 5.00, max_slots: 3 }
              │
              ▼
Supabase insert:
    INSERT INTO tasks (id, bounty_amount, max_slots, …)
    VALUES ('a3f9-4b2c…', 5, 3, …) RETURNING id;
              │
              ▼
Frontend signs program call:
    initialize_task(
      task_id:             uuid_to_bytes('a3f9-4b2c…'),
      bounty:              5_000_000,          // $5 in micro-USDC
      max_slots:           3,
      claim_deadline_secs: 172800,             // 48h submission window
      task_deadline_ts:    1_729_000_000,      // 0 = no task-level deadline
    )
              │
              ▼
On tx confirmation, backend updates DB:
    UPDATE tasks
    SET escrow_program_account = <TaskPDA pubkey>,
        escrow_tx_signature    = <sig>,
        payment_status         = 'deposited'
    WHERE id = 'a3f9-4b2c…';
```

### The vault: separate account from the task struct

```
EscrowTask PDA                         TaskVault ATA
(seeds = [b"task", task_id])           (owner = EscrowTask PDA, mint = USDC)
┌────────────────────┐                ┌────────────────────────────┐
│ poster: 0xa1b2…    │                │ mint: EPjFWdd5… (USDC)     │
│ bounty: 5_000_000  │                │ owner: EscrowTask PDA      │
│ max_slots: 3       │                │ amount: 15_000_000         │
│ slots_approved: 0  │  ────────────► │ (holds the actual USDC)    │
│ status: Open       │                │                            │
└────────────────────┘                └────────────────────────────┘
 "metadata"                            "money"
```

The struct is the state machine. The vault holds the tokens. They're linked by
ATA derivation: `TaskVault = ATA(TaskPDA, USDC_mint)`. The vault's only
authority is the task PDA, for which no private key exists.

### Payout math (on approval)

```
net  = bounty × (10000 - fee_bps) / 10000        // 4_800_000 = $4.80
fee  = bounty - net                              //   200_000 = $0.20

task_vault ─ $4.80 ─► claimer's USDC ATA
task_vault ─ $0.20 ─► fee_vault (owned by config PDA)
slots_approved++
```

## Safety invariants

1. **No off-chain fund authority.** The vault PDA has no private key; the
   program is the sole authority.
2. **Per-task isolation.** A compromised claim in task A cannot drain task B.
3. **Integer math only.** `u64` micro-USDC everywhere, no floats, no rounding
   drift.
4. **Signer checks on every mutating instruction.** `has_one` + `Signer`
   constraints enforce the right party at every step.
5. **Mint lock.** All CPIs assert `task_vault.mint == config.usdc_mint`;
   wrong-token deposits are rejected.
6. **Bounded fee.** Admin cannot set `fee_bps > 500` (5%) in-program. Ever.
7. **Fee vault isolation.** A compromised admin can only drain accumulated
   fees, not any task's vault. Verified by the `task_vault as fee_vault` test.
8. **Time gates via `Clock` sysvar.** Auto-actions use consensus-agreed time,
   not off-chain clocks.
9. **Replay-safe.** PDA derivation from `task_id` / `claim_id` prevents
   double-init; Solana's `recent_blockhash` prevents tx replay.
10. **State machine checks.** Every mutating instruction `require!`s the
    current state; wrong-state calls are rejected with `InvalidStateTransition`.

## Known constraints & product decisions

- **Bounty frozen at init.** No `update_bounty` instruction. Changing escrowed
  amounts mid-flight would open race conditions. If a poster wants to raise
  the bounty, they cancel and re-create.
- **Uniform bounty per slot.** All slots in a multi-slot task pay the same
  amount. No tiered payouts. (Could be revisited in v2 if needed.)
- **One SPL mint per deployment.** Pinned at `initialize_config`. To support a
  different stablecoin, deploy a second program.
- **3-rejection cap.** Hard-coded to prevent poster from griefing a deliverer
  indefinitely.
- **14-day auto-action windows.** Hard-coded in `state.rs`. Changes require
  program upgrade.
- **5% fee cap.** Hard-coded. Admin cannot exceed without source-level change +
  program upgrade.

## When things go wrong — the recovery paths

| Situation | Recovery path |
|---|---|
| Poster ghosts after depositing, nobody enrolls | Anyone calls `auto_cancel_task` after `task_deadline_ts` — poster gets refund |
| Poster ghosts after some slots filled | Anyone calls `close_task` after `task_deadline_ts` — poster gets unused-slot refund |
| Poster ghosts on a Submitted claim | Anyone calls `auto_approve` after 14 days — deliverer gets paid |
| Admin ghosts on an open dispute | Anyone calls `auto_resolve_dispute` after 14 days — deliverer wins by default |
| Poster rejects unfairly | Deliverer calls `open_dispute`; admin resolves, or auto-resolves in 14d |
| Admin key compromised | Current admin (if they notice first) calls `update_admin` to rotate. Worst case: program upgrade authority (Squads multisig at mainnet) can deploy a patched version. |
| Program upgrade authority compromised | This is the worst case. Mainnet must use a Squads multisig so a single key compromise is insufficient. |

## Links

- Source: [`programs/clustly-escrow/src/`](../programs/clustly-escrow/src/)
- Tests: [`tests/escrow.ts`](../tests/escrow.ts) (47 tests, 28 adversarial)
- Security audit: [`../../tasks/solana-escrow-security-audit.md`](../../tasks/solana-escrow-security-audit.md)
- Devnet deploy: [`../../tasks/solana-escrow-devnet-results.md`](../../tasks/solana-escrow-devnet-results.md)
- Followup wiring plan: [`wiring-plan.md`](wiring-plan.md)
