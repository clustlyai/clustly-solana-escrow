use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::associated_token::AssociatedToken;

use crate::errors::EscrowError;
use crate::state::*;

/* ── Enroll ───────────────────────────────────────────── */

#[derive(Accounts)]
#[instruction(claim_id: [u8; 16])]
pub struct Enroll<'info> {
    #[account(mut)]
    pub claimer: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, EscrowConfig>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
    )]
    pub task: Box<Account<'info, EscrowTask>>,

    #[account(
        init,
        payer = claimer,
        space = 8 + EscrowClaim::SIZE,
        seeds = [CLAIM_SEED, &claim_id],
        bump,
    )]
    pub claim: Box<Account<'info, EscrowClaim>>,

    // Claimer's USDC ATA is created here (at enroll) so the claimer pays
    // their own account rent. Previously this was created in approve_claim
    // with the poster as payer, silently subsidizing the claimer's account
    // creation. Claimer signs enroll, so they can (and should) pay.
    #[account(
        init_if_needed,
        payer = claimer,
        associated_token::mint = usdc_mint,
        associated_token::authority = claimer,
    )]
    pub claimer_ata: Box<Account<'info, TokenAccount>>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn enroll(ctx: Context<Enroll>, claim_id: [u8; 16]) -> Result<()> {
    let task = &mut ctx.accounts.task;
    require!(
        task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );
    require!(
        task.slots_enrolled < task.max_slots,
        EscrowError::SlotsFull
    );
    require!(
        ctx.accounts.claimer.key() != task.poster,
        EscrowError::SelfEnrollment
    );

    let claim = &mut ctx.accounts.claim;
    claim.claim_id = claim_id;
    claim.task = task.key();
    claim.claimer = ctx.accounts.claimer.key();
    claim.set_status(ClaimStatus::Enrolled);
    claim.enrolled_at = Clock::get()?.unix_timestamp;
    claim.submitted_at = 0;
    claim.disputed_at = 0;
    claim.rejections = 0;
    claim.deliverable_hash = [0u8; 32];
    claim.bump = ctx.bumps.claim;
    claim._reserved = [0u8; 16];

    task.slots_enrolled = task
        .slots_enrolled
        .checked_add(1)
        .ok_or(EscrowError::MathOverflow)?;

    msg!(
        "Enrolled: claim_id={:?} task={} claimer={} slot={}/{}",
        claim_id,
        task.key(),
        claim.claimer,
        task.slots_enrolled,
        task.max_slots
    );
    Ok(())
}

/* ── Submit (accepts Enrolled OR Rejected = resubmit) ─── */

#[derive(Accounts)]
pub struct Submit<'info> {
    pub claimer: Signer<'info>,

    #[account(
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
    )]
    pub task: Account<'info, EscrowTask>,

    #[account(
        mut,
        seeds = [CLAIM_SEED, &claim.claim_id],
        bump = claim.bump,
        has_one = claimer @ EscrowError::Unauthorized,
        has_one = task @ EscrowError::TaskClaimMismatch,
    )]
    pub claim: Account<'info, EscrowClaim>,
}

pub fn submit(ctx: Context<Submit>, deliverable_hash: [u8; 32]) -> Result<()> {
    let claim = &mut ctx.accounts.claim;
    let current = claim.status()?;
    require!(
        current == ClaimStatus::Enrolled || current == ClaimStatus::Rejected,
        EscrowError::InvalidStateTransition
    );

    claim.set_status(ClaimStatus::Submitted);
    claim.submitted_at = Clock::get()?.unix_timestamp;
    claim.deliverable_hash = deliverable_hash;

    msg!(
        "Submitted: claim={} (prior={:?}, rejections={})",
        claim.key(),
        current,
        claim.rejections
    );
    Ok(())
}

/* ── Approve claim ────────────────────────────────────── */

#[derive(Accounts)]
pub struct ApproveClaim<'info> {
    #[account(mut)]
    pub poster: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, EscrowConfig>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
        has_one = poster @ EscrowError::Unauthorized,
    )]
    pub task: Box<Account<'info, EscrowTask>>,

    #[account(
        mut,
        seeds = [CLAIM_SEED, &claim.claim_id],
        bump = claim.bump,
        has_one = task @ EscrowError::TaskClaimMismatch,
    )]
    pub claim: Box<Account<'info, EscrowClaim>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Box<Account<'info, TokenAccount>>,

    // Deliverer's USDC ATA. Pre-created by the claimer during enroll, so
    // the poster does NOT pay rent for the claimer's account. If missing
    // here, it means the claim never signed enroll() on-chain — the tx
    // will fail, reconciler can heal.
    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = claimer,
    )]
    pub claimer_ata: Box<Account<'info, TokenAccount>>,

    /// CHECK: must match claim.claimer — used as the ATA authority.
    #[account(address = claim.claimer @ EscrowError::Unauthorized)]
    pub claimer: UncheckedAccount<'info>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = config,
    )]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn approve_claim(ctx: Context<ApproveClaim>) -> Result<()> {
    require!(
        ctx.accounts.usdc_mint.key() == ctx.accounts.config.usdc_mint,
        EscrowError::InvalidMint
    );

    require!(
        ctx.accounts.task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );
    require!(
        ctx.accounts.claim.status()? == ClaimStatus::Submitted,
        EscrowError::InvalidStateTransition
    );

    let bounty = ctx.accounts.task.bounty;
    let fee_bps = ctx.accounts.config.fee_bps;
    let (net, fee) = compute_fee_split(bounty, fee_bps)?;

    let task_id = ctx.accounts.task.task_id;
    let task_bump = ctx.accounts.task.bump;
    let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[task_bump]];
    let signer_seeds = &[seeds];

    let task_info = ctx.accounts.task.to_account_info();
    let token_program = ctx.accounts.token_program.to_account_info();
    let task_vault = ctx.accounts.task_vault.to_account_info();
    let claimer_ata = ctx.accounts.claimer_ata.to_account_info();
    let fee_vault = ctx.accounts.fee_vault.to_account_info();

    if net > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program.clone(),
            Transfer {
                from: task_vault.clone(),
                to: claimer_ata,
                authority: task_info.clone(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, net)?;
    }

    if fee > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program,
            Transfer {
                from: task_vault,
                to: fee_vault,
                authority: task_info,
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, fee)?;
    }

    let task = &mut ctx.accounts.task;
    let claim = &mut ctx.accounts.claim;
    claim.set_status(ClaimStatus::Approved);
    task.slots_approved = task
        .slots_approved
        .checked_add(1)
        .ok_or(EscrowError::MathOverflow)?;

    msg!(
        "Approved: claim={} net={} fee={} slots_approved={}/{}",
        claim.key(),
        net,
        fee,
        task.slots_approved,
        task.max_slots
    );
    Ok(())
}

/* ── Reject claim ─────────────────────────────────────── */

#[derive(Accounts)]
pub struct RejectClaim<'info> {
    #[account(mut)]
    pub poster: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, EscrowConfig>,

    #[account(
        mut,
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
        has_one = poster @ EscrowError::Unauthorized,
    )]
    pub task: Account<'info, EscrowTask>,

    #[account(
        mut,
        seeds = [CLAIM_SEED, &claim.claim_id],
        bump = claim.bump,
        has_one = task @ EscrowError::TaskClaimMismatch,
    )]
    pub claim: Account<'info, EscrowClaim>,

    #[account(
        mut,
        associated_token::mint = config.usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Account<'info, TokenAccount>,

    /// Needed only if rejections hits the cap and we refund that slot to poster.
    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = task.poster,
    )]
    pub poster_ata: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn reject_claim(ctx: Context<RejectClaim>) -> Result<()> {
    require!(
        ctx.accounts.task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );
    require!(
        ctx.accounts.claim.status()? == ClaimStatus::Submitted,
        EscrowError::InvalidStateTransition
    );

    let new_rejections = ctx
        .accounts
        .claim
        .rejections
        .checked_add(1)
        .ok_or(EscrowError::MathOverflow)?;

    if new_rejections >= MAX_REJECTIONS {
        let refund = ctx.accounts.task.bounty;
        let task_id = ctx.accounts.task.task_id;
        let task_bump = ctx.accounts.task.bump;
        let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[task_bump]];
        let signer_seeds = &[seeds];

        let task_info = ctx.accounts.task.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.task_vault.to_account_info(),
                to: ctx.accounts.poster_ata.to_account_info(),
                authority: task_info,
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, refund)?;

        let task = &mut ctx.accounts.task;
        let claim = &mut ctx.accounts.claim;
        claim.rejections = new_rejections;
        claim.set_status(ClaimStatus::Cancelled);
        task.slots_refunded = task
            .slots_refunded
            .checked_add(1)
            .ok_or(EscrowError::MathOverflow)?;

        msg!(
            "Claim cancelled after {} rejections. Refunded {} to poster",
            new_rejections,
            refund
        );
    } else {
        let claim = &mut ctx.accounts.claim;
        claim.rejections = new_rejections;
        claim.set_status(ClaimStatus::Rejected);
        msg!(
            "Rejected: claim={} ({}/{} rejections)",
            claim.key(),
            new_rejections,
            MAX_REJECTIONS
        );
    }

    Ok(())
}

/* ── Auto-approve (permissionless crank, 14d+) ─────────── */

#[derive(Accounts)]
pub struct AutoApprove<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, EscrowConfig>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
    )]
    pub task: Box<Account<'info, EscrowTask>>,

    #[account(
        mut,
        seeds = [CLAIM_SEED, &claim.claim_id],
        bump = claim.bump,
        has_one = task @ EscrowError::TaskClaimMismatch,
    )]
    pub claim: Box<Account<'info, EscrowClaim>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Box<Account<'info, TokenAccount>>,

    // Must already exist (pre-created at enroll). Crank caller can't pay ATA rent.
    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = claimer,
    )]
    pub claimer_ata: Box<Account<'info, TokenAccount>>,

    /// CHECK: must match claim.claimer
    #[account(address = claim.claimer @ EscrowError::Unauthorized)]
    pub claimer: UncheckedAccount<'info>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = config,
    )]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn auto_approve(ctx: Context<AutoApprove>) -> Result<()> {
    require!(
        ctx.accounts.usdc_mint.key() == ctx.accounts.config.usdc_mint,
        EscrowError::InvalidMint
    );

    require!(
        ctx.accounts.task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );
    require!(
        ctx.accounts.claim.status()? == ClaimStatus::Submitted,
        EscrowError::InvalidStateTransition
    );

    let now = Clock::get()?.unix_timestamp;
    let submitted_at = ctx.accounts.claim.submitted_at;
    require!(
        submitted_at > 0 && now.saturating_sub(submitted_at) >= AUTO_APPROVE_WINDOW_SECS,
        EscrowError::AutoActionTooEarly
    );

    let bounty = ctx.accounts.task.bounty;
    let fee_bps = ctx.accounts.config.fee_bps;
    let (net, fee) = compute_fee_split(bounty, fee_bps)?;

    let task_id = ctx.accounts.task.task_id;
    let task_bump = ctx.accounts.task.bump;
    let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[task_bump]];
    let signer_seeds = &[seeds];

    let task_info = ctx.accounts.task.to_account_info();
    let token_program = ctx.accounts.token_program.to_account_info();
    let task_vault = ctx.accounts.task_vault.to_account_info();
    let claimer_ata = ctx.accounts.claimer_ata.to_account_info();
    let fee_vault = ctx.accounts.fee_vault.to_account_info();

    if net > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program.clone(),
            Transfer {
                from: task_vault.clone(),
                to: claimer_ata,
                authority: task_info.clone(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, net)?;
    }

    if fee > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program,
            Transfer {
                from: task_vault,
                to: fee_vault,
                authority: task_info,
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, fee)?;
    }

    let task = &mut ctx.accounts.task;
    let claim = &mut ctx.accounts.claim;
    claim.set_status(ClaimStatus::Approved);
    task.slots_approved = task
        .slots_approved
        .checked_add(1)
        .ok_or(EscrowError::MathOverflow)?;

    msg!(
        "Auto-approved: claim={} net={} fee={} (elapsed={}s)",
        claim.key(),
        net,
        fee,
        now - submitted_at
    );
    Ok(())
}
