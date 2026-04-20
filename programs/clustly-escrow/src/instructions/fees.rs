use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

use crate::errors::EscrowError;
use crate::state::*;

/// Withdraw accumulated fees from the dedicated fee_vault to any destination.
///
/// The fee_vault is architecturally separate from any task vault, so even a
/// fully-compromised admin can only drain whatever fees have accumulated —
/// they cannot touch in-flight escrow money sitting in task vaults.
#[derive(Accounts)]
pub struct WithdrawFees<'info> {
    pub admin: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ EscrowError::Unauthorized,
    )]
    pub config: Account<'info, EscrowConfig>,

    #[account(
        mut,
        associated_token::mint = config.usdc_mint,
        associated_token::authority = config,
    )]
    pub fee_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = config.usdc_mint,
    )]
    pub destination: Account<'info, TokenAccount>,

    pub usdc_mint: Account<'info, Mint>,

    pub token_program: Program<'info, Token>,
}

pub fn withdraw_fees(ctx: Context<WithdrawFees>, amount: u64) -> Result<()> {
    require!(
        ctx.accounts.fee_vault.amount >= amount,
        EscrowError::InsufficientFeeVault
    );

    let bump = ctx.accounts.config.bump;
    let seeds: &[&[u8]] = &[CONFIG_SEED, &[bump]];
    let signer_seeds = &[seeds];

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.fee_vault.to_account_info(),
            to: ctx.accounts.destination.to_account_info(),
            authority: ctx.accounts.config.to_account_info(),
        },
        signer_seeds,
    );
    token::transfer(cpi_ctx, amount)?;

    msg!("Withdrew {} fees to {}", amount, ctx.accounts.destination.key());
    Ok(())
}
