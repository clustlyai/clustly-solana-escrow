// Clustly freelancing escrow program (Solana + SPL USDC).
//
// See `../../../proud-humming-shell.md` for the design; high-level rules:
//   - Custody is per-task: each task owns its own SPL token vault via PDA.
//   - Fees (4%) auto-route into a dedicated program-owned fee_vault in the
//     SAME instruction as the deliverer payout (approve_claim / auto_approve /
//     resolve_dispute(Deliverer)). Task vaults NEVER hold fee post-approval.
//   - Admin can only withdraw from fee_vault. Compromised admin cannot drain
//     task vaults.
//   - Hard cap on fee_bps = 500 (5%). Enforced in program.
//   - State-machine guards on every mutating instruction.

use anchor_lang::prelude::*;

pub mod errors;
pub mod state;
pub mod instructions;

use instructions::*;

declare_id!("8pnWWX45FL6WmzP38bfrxjxKKAkwwWPUhkP7iRwarv8K");

#[program]
pub mod clustly_escrow {
    use super::*;

    /* ── Config ───────────────────────────────────────────── */

    pub fn initialize_config(
        ctx: Context<InitializeConfig>,
        admin: Pubkey,
        fee_bps: u16,
    ) -> Result<()> {
        instructions::config::initialize_config(ctx, admin, fee_bps)
    }

    pub fn update_admin(ctx: Context<UpdateAdmin>, new_admin: Pubkey) -> Result<()> {
        instructions::config::update_admin(ctx, new_admin)
    }

    pub fn update_fee(ctx: Context<UpdateFee>, new_fee_bps: u16) -> Result<()> {
        instructions::config::update_fee(ctx, new_fee_bps)
    }

    pub fn update_mint(ctx: Context<UpdateMint>) -> Result<()> {
        instructions::config::update_mint(ctx)
    }

    /* ── Task lifecycle ──────────────────────────────────── */

    pub fn initialize_task(
        ctx: Context<InitializeTask>,
        task_id: [u8; 16],
        bounty: u64,
        max_slots: u16,
        claim_deadline_secs: u32,
        task_deadline_ts: i64,
    ) -> Result<()> {
        instructions::task::initialize_task(
            ctx,
            task_id,
            bounty,
            max_slots,
            claim_deadline_secs,
            task_deadline_ts,
        )
    }

    pub fn cancel_before_enroll(ctx: Context<CancelBeforeEnroll>) -> Result<()> {
        instructions::task::cancel_before_enroll(ctx)
    }

    pub fn close_task(ctx: Context<CloseTask>) -> Result<()> {
        instructions::task::close_task(ctx)
    }

    pub fn auto_cancel_task(ctx: Context<AutoCancelTask>) -> Result<()> {
        instructions::task::auto_cancel_task(ctx)
    }

    /* ── Claim lifecycle ─────────────────────────────────── */

    pub fn enroll(ctx: Context<Enroll>, claim_id: [u8; 16]) -> Result<()> {
        instructions::claim::enroll(ctx, claim_id)
    }

    pub fn submit(ctx: Context<Submit>, deliverable_hash: [u8; 32]) -> Result<()> {
        instructions::claim::submit(ctx, deliverable_hash)
    }

    pub fn approve_claim(ctx: Context<ApproveClaim>) -> Result<()> {
        instructions::claim::approve_claim(ctx)
    }

    pub fn reject_claim(ctx: Context<RejectClaim>) -> Result<()> {
        instructions::claim::reject_claim(ctx)
    }

    pub fn auto_approve(ctx: Context<AutoApprove>) -> Result<()> {
        instructions::claim::auto_approve(ctx)
    }

    /* ── Dispute ─────────────────────────────────────────── */

    pub fn open_dispute(ctx: Context<OpenDispute>) -> Result<()> {
        instructions::dispute::open_dispute(ctx)
    }

    pub fn resolve_dispute(ctx: Context<ResolveDispute>, favor: DisputeFavor) -> Result<()> {
        instructions::dispute::resolve_dispute(ctx, favor)
    }

    pub fn auto_resolve_dispute(ctx: Context<AutoResolveDispute>) -> Result<()> {
        instructions::dispute::auto_resolve_dispute(ctx)
    }

    /* ── Fees ────────────────────────────────────────────── */

    pub fn withdraw_fees(ctx: Context<WithdrawFees>, amount: u64) -> Result<()> {
        instructions::fees::withdraw_fees(ctx, amount)
    }
}
