use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::associated_token::AssociatedToken;

use crate::errors::EscrowError;
use crate::state::*;

/* ── Open dispute (claimer only, from Rejected) ───────── */

#[derive(Accounts)]
pub struct OpenDispute<'info> {
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

pub fn open_dispute(ctx: Context<OpenDispute>) -> Result<()> {
    let claim = &mut ctx.accounts.claim;
    require!(
        claim.status()? == ClaimStatus::Rejected,
        EscrowError::InvalidStateTransition
    );

    claim.set_status(ClaimStatus::Disputed);
    claim.disputed_at = Clock::get()?.unix_timestamp;

    msg!("Dispute opened: claim={}", claim.key());
    Ok(())
}

/* ── Resolve dispute (admin only) ─────────────────────── */

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum DisputeFavor {
    Deliverer = 0,
    Poster = 1,
}

#[derive(Accounts)]
pub struct ResolveDispute<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ EscrowError::Unauthorized,
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

    /// CHECK: must match claim.claimer
    #[account(address = claim.claimer @ EscrowError::Unauthorized)]
    pub claimer: UncheckedAccount<'info>,

    // Pre-created at enroll; admin does NOT pay rent. If missing, tx fails.
    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = claimer,
    )]
    pub claimer_ata: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = usdc_mint,
        token::authority = task.poster,
    )]
    pub poster_ata: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = config,
    )]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn resolve_dispute(ctx: Context<ResolveDispute>, favor: DisputeFavor) -> Result<()> {
    require!(
        ctx.accounts.usdc_mint.key() == ctx.accounts.config.usdc_mint,
        EscrowError::InvalidMint
    );

    require!(
        ctx.accounts.claim.status()? == ClaimStatus::Disputed,
        EscrowError::InvalidStateTransition
    );

    let bounty = ctx.accounts.task.bounty;
    let fee_bps = ctx.accounts.config.fee_bps;
    let task_id = ctx.accounts.task.task_id;
    let task_bump = ctx.accounts.task.bump;
    let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[task_bump]];
    let signer_seeds = &[seeds];

    let task_info = ctx.accounts.task.to_account_info();
    let token_program = ctx.accounts.token_program.to_account_info();
    let task_vault = ctx.accounts.task_vault.to_account_info();
    let claimer_ata = ctx.accounts.claimer_ata.to_account_info();
    let poster_ata = ctx.accounts.poster_ata.to_account_info();
    let fee_vault = ctx.accounts.fee_vault.to_account_info();

    match favor {
        DisputeFavor::Deliverer => {
            let (net, fee) = compute_fee_split(bounty, fee_bps)?;
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
            claim.set_status(ClaimStatus::ResolvedDeliverer);
            task.slots_approved = task
                .slots_approved
                .checked_add(1)
                .ok_or(EscrowError::MathOverflow)?;
            msg!("Dispute resolved (deliverer): net={} fee={}", net, fee);
        }
        DisputeFavor::Poster => {
            let refund = bounty;
            let cpi_ctx = CpiContext::new_with_signer(
                token_program,
                Transfer {
                    from: task_vault,
                    to: poster_ata,
                    authority: task_info,
                },
                signer_seeds,
            );
            token::transfer(cpi_ctx, refund)?;
            let task = &mut ctx.accounts.task;
            let claim = &mut ctx.accounts.claim;
            claim.set_status(ClaimStatus::ResolvedPoster);
            task.slots_refunded = task
                .slots_refunded
                .checked_add(1)
                .ok_or(EscrowError::MathOverflow)?;
            msg!("Dispute resolved (poster): refund={}", refund);
        }
    }

    Ok(())
}

/* ── Auto-resolve dispute (permissionless, 14d+, deliverer wins) ─ */

#[derive(Accounts)]
pub struct AutoResolveDispute<'info> {
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

    /// CHECK: must match claim.claimer
    #[account(address = claim.claimer @ EscrowError::Unauthorized)]
    pub claimer: UncheckedAccount<'info>,

    // Pre-created at enroll; crank caller does NOT pay rent.
    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = claimer,
    )]
    pub claimer_ata: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = usdc_mint,
        associated_token::authority = config,
    )]
    pub fee_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn auto_resolve_dispute(ctx: Context<AutoResolveDispute>) -> Result<()> {
    require!(
        ctx.accounts.usdc_mint.key() == ctx.accounts.config.usdc_mint,
        EscrowError::InvalidMint
    );

    require!(
        ctx.accounts.claim.status()? == ClaimStatus::Disputed,
        EscrowError::InvalidStateTransition
    );

    let now = Clock::get()?.unix_timestamp;
    let disputed_at = ctx.accounts.claim.disputed_at;
    require!(
        disputed_at > 0 && now.saturating_sub(disputed_at) >= AUTO_DISPUTE_WINDOW_SECS,
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
    claim.set_status(ClaimStatus::ResolvedDeliverer);
    task.slots_approved = task
        .slots_approved
        .checked_add(1)
        .ok_or(EscrowError::MathOverflow)?;

    msg!(
        "Dispute auto-resolved (deliverer): net={} fee={} elapsed={}s",
        net,
        fee,
        now - disputed_at
    );
    Ok(())
}
