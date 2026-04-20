use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use anchor_spl::associated_token::AssociatedToken;

use crate::errors::EscrowError;
use crate::state::*;

#[derive(Accounts)]
#[instruction(task_id: [u8; 16], bounty: u64, max_slots: u16)]
pub struct InitializeTask<'info> {
    #[account(mut)]
    pub poster: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, EscrowConfig>,

    #[account(
        address = config.usdc_mint @ EscrowError::InvalidMint
    )]
    pub usdc_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = poster,
        space = 8 + EscrowTask::SIZE,
        seeds = [TASK_SEED, &task_id],
        bump,
    )]
    pub task: Account<'info, EscrowTask>,

    #[account(
        init,
        payer = poster,
        associated_token::mint = usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = usdc_mint,
        token::authority = poster,
    )]
    pub poster_ata: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn initialize_task(
    ctx: Context<InitializeTask>,
    task_id: [u8; 16],
    bounty: u64,
    max_slots: u16,
    claim_deadline_secs: u32,
    task_deadline_ts: i64,
) -> Result<()> {
    require!(bounty > 0, EscrowError::ZeroBounty);
    require!(
        max_slots >= 1 && max_slots <= MAX_SLOTS_LIMIT,
        EscrowError::InvalidSlots
    );

    // Compute bounty * max_slots checked against both u128 overflow AND u64 fit.
    // `as u64` would silently truncate if the u128 result exceeds u64::MAX, letting
    // a malicious poster escrow less than `bounty * max_slots`. We force an
    // explicit MathOverflow in both cases so no tokens move.
    let total_u128 = (bounty as u128)
        .checked_mul(max_slots as u128)
        .ok_or(EscrowError::MathOverflow)?;
    let total: u64 = total_u128
        .try_into()
        .map_err(|_| EscrowError::MathOverflow)?;

    let task = &mut ctx.accounts.task;
    task.task_id = task_id;
    task.poster = ctx.accounts.poster.key();
    task.bounty = bounty;
    task.max_slots = max_slots;
    task.slots_enrolled = 0;
    task.slots_approved = 0;
    task.slots_refunded = 0;
    task.set_status(TaskStatus::Open);
    task.task_deadline_ts = task_deadline_ts;
    task.claim_deadline_secs = claim_deadline_secs;
    task.bump = ctx.bumps.task;
    task.vault_bump = 0;
    task._reserved = [0u8; 32];

    // Transfer `bounty * max_slots` USDC from poster to task_vault.
    let cpi_ctx = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.poster_ata.to_account_info(),
            to: ctx.accounts.task_vault.to_account_info(),
            authority: ctx.accounts.poster.to_account_info(),
        },
    );
    token::transfer(cpi_ctx, total)?;

    msg!(
        "Task initialized: id={:?} bounty={} slots={} total_deposited={}",
        task_id,
        bounty,
        max_slots,
        total
    );
    Ok(())
}

/* ── Cancel before enroll ────────────────────────────── */

#[derive(Accounts)]
pub struct CancelBeforeEnroll<'info> {
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
        close = poster,
    )]
    pub task: Account<'info, EscrowTask>,

    #[account(
        mut,
        associated_token::mint = config.usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = poster,
    )]
    pub poster_ata: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn cancel_before_enroll(ctx: Context<CancelBeforeEnroll>) -> Result<()> {
    require!(
        ctx.accounts.task.slots_enrolled == 0,
        EscrowError::TaskHasEnrollments
    );
    require!(
        ctx.accounts.task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );

    let refund = ctx.accounts.task_vault.amount;
    let task_id = ctx.accounts.task.task_id;
    let bump = ctx.accounts.task.bump;
    let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[bump]];
    let signer_seeds = &[seeds];

    let task_info = ctx.accounts.task.to_account_info();
    let token_program = ctx.accounts.token_program.to_account_info();
    let task_vault = ctx.accounts.task_vault.to_account_info();
    let poster_ata = ctx.accounts.poster_ata.to_account_info();
    let poster = ctx.accounts.poster.to_account_info();

    if refund > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program.clone(),
            Transfer {
                from: task_vault.clone(),
                to: poster_ata,
                authority: task_info.clone(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, refund)?;
    }

    let close_ctx = CpiContext::new_with_signer(
        token_program,
        token::CloseAccount {
            account: task_vault,
            destination: poster,
            authority: task_info,
        },
        signer_seeds,
    );
    token::close_account(close_ctx)?;

    msg!("Task cancelled, refunded {} to poster", refund);
    Ok(())
}

/* ── Close task (poster OR permissionless-after-deadline) ─── */

#[derive(Accounts)]
pub struct CloseTask<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, EscrowConfig>,

    #[account(
        mut,
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
    )]
    pub task: Account<'info, EscrowTask>,

    #[account(
        mut,
        associated_token::mint = config.usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Account<'info, TokenAccount>,

    /// Poster's USDC ATA, where the remaining (unfilled / refunded) bounty goes.
    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = task.poster,
    )]
    pub poster_ata: Account<'info, TokenAccount>,

    /// CHECK: Must match task.poster. Used only for the rent-refund recipient on vault close.
    #[account(mut, address = task.poster @ EscrowError::Unauthorized)]
    pub poster: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

pub fn close_task(ctx: Context<CloseTask>) -> Result<()> {
    let is_poster = ctx.accounts.caller.key() == ctx.accounts.task.poster;
    if !is_poster {
        let now = Clock::get()?.unix_timestamp;
        require!(
            ctx.accounts.task.task_deadline_ts > 0 && now >= ctx.accounts.task.task_deadline_ts,
            EscrowError::DeadlineNotReached
        );
    }

    require!(
        ctx.accounts.task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );

    let slots_enrolled = ctx.accounts.task.slots_enrolled;
    let slots_approved = ctx.accounts.task.slots_approved;
    let slots_refunded = ctx.accounts.task.slots_refunded;
    let active_slots = slots_enrolled
        .saturating_sub(slots_approved)
        .saturating_sub(slots_refunded);
    require!(active_slots == 0, EscrowError::ClaimsNotResolved);

    let remaining = ctx.accounts.task_vault.amount;
    let task_id = ctx.accounts.task.task_id;
    let bump = ctx.accounts.task.bump;
    let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[bump]];
    let signer_seeds = &[seeds];

    let task_info = ctx.accounts.task.to_account_info();
    let token_program = ctx.accounts.token_program.to_account_info();
    let task_vault = ctx.accounts.task_vault.to_account_info();
    let poster_ata = ctx.accounts.poster_ata.to_account_info();
    let poster = ctx.accounts.poster.to_account_info();

    if remaining > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program.clone(),
            Transfer {
                from: task_vault.clone(),
                to: poster_ata,
                authority: task_info.clone(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, remaining)?;
    }

    let close_ctx = CpiContext::new_with_signer(
        token_program,
        token::CloseAccount {
            account: task_vault,
            destination: poster,
            authority: task_info,
        },
        signer_seeds,
    );
    token::close_account(close_ctx)?;

    let task = &mut ctx.accounts.task;
    task.set_status(TaskStatus::Closed);
    msg!("Task closed. Refunded {} to poster", remaining);
    Ok(())
}

/* ── Auto-cancel: no enrollments after task_deadline ───── */

#[derive(Accounts)]
pub struct AutoCancelTask<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, EscrowConfig>,

    #[account(
        mut,
        seeds = [TASK_SEED, &task.task_id],
        bump = task.bump,
    )]
    pub task: Account<'info, EscrowTask>,

    #[account(
        mut,
        associated_token::mint = config.usdc_mint,
        associated_token::authority = task,
    )]
    pub task_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = config.usdc_mint,
        token::authority = task.poster,
    )]
    pub poster_ata: Account<'info, TokenAccount>,

    /// CHECK: rent refund recipient
    #[account(mut, address = task.poster @ EscrowError::Unauthorized)]
    pub poster: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

pub fn auto_cancel_task(ctx: Context<AutoCancelTask>) -> Result<()> {
    require!(
        ctx.accounts.task.slots_enrolled == 0,
        EscrowError::TaskHasEnrollments
    );
    require!(
        ctx.accounts.task.status() == TaskStatus::Open,
        EscrowError::InvalidStateTransition
    );

    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.task.task_deadline_ts > 0 && now >= ctx.accounts.task.task_deadline_ts,
        EscrowError::DeadlineNotReached
    );

    let refund = ctx.accounts.task_vault.amount;
    let task_id = ctx.accounts.task.task_id;
    let bump = ctx.accounts.task.bump;
    let seeds: &[&[u8]] = &[TASK_SEED, &task_id, &[bump]];
    let signer_seeds = &[seeds];

    let task_info = ctx.accounts.task.to_account_info();
    let token_program = ctx.accounts.token_program.to_account_info();
    let task_vault = ctx.accounts.task_vault.to_account_info();
    let poster_ata = ctx.accounts.poster_ata.to_account_info();
    let poster = ctx.accounts.poster.to_account_info();

    if refund > 0 {
        let cpi_ctx = CpiContext::new_with_signer(
            token_program.clone(),
            Transfer {
                from: task_vault.clone(),
                to: poster_ata,
                authority: task_info.clone(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, refund)?;
    }

    let close_ctx = CpiContext::new_with_signer(
        token_program,
        token::CloseAccount {
            account: task_vault,
            destination: poster,
            authority: task_info,
        },
        signer_seeds,
    );
    token::close_account(close_ctx)?;

    let task = &mut ctx.accounts.task;
    task.set_status(TaskStatus::Closed);
    msg!("Task auto-cancelled. Refunded {} to poster", refund);
    Ok(())
}
