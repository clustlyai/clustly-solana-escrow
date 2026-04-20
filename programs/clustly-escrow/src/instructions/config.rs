use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};
use anchor_spl::associated_token::AssociatedToken;

use crate::errors::EscrowError;
use crate::state::*;

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        init,
        payer = payer,
        space = 8 + EscrowConfig::SIZE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, EscrowConfig>,

    pub usdc_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = payer,
        associated_token::mint = usdc_mint,
        associated_token::authority = config,
    )]
    pub fee_vault: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn initialize_config(
    ctx: Context<InitializeConfig>,
    admin: Pubkey,
    fee_bps: u16,
) -> Result<()> {
    require!(fee_bps <= FEE_BPS_CAP, EscrowError::FeeTooHigh);

    let config = &mut ctx.accounts.config;
    config.admin = admin;
    config.usdc_mint = ctx.accounts.usdc_mint.key();
    config.fee_bps = fee_bps;
    config.bump = ctx.bumps.config;
    config.fee_vault_bump = 0; // fee vault is an ATA, bump not applicable
    config._reserved = [0u8; 64];

    msg!(
        "Config initialized: admin={} mint={} fee_bps={}",
        admin,
        config.usdc_mint,
        fee_bps
    );
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateAdmin<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ EscrowError::Unauthorized,
    )]
    pub config: Account<'info, EscrowConfig>,

    pub admin: Signer<'info>,
}

pub fn update_admin(ctx: Context<UpdateAdmin>, new_admin: Pubkey) -> Result<()> {
    let config = &mut ctx.accounts.config;
    msg!("Admin rotated from {} to {}", config.admin, new_admin);
    config.admin = new_admin;
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateFee<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ EscrowError::Unauthorized,
    )]
    pub config: Account<'info, EscrowConfig>,

    pub admin: Signer<'info>,
}

pub fn update_fee(ctx: Context<UpdateFee>, new_fee_bps: u16) -> Result<()> {
    require!(new_fee_bps <= FEE_BPS_CAP, EscrowError::FeeTooHigh);
    let config = &mut ctx.accounts.config;
    msg!("Fee bps updated from {} to {}", config.fee_bps, new_fee_bps);
    config.fee_bps = new_fee_bps;
    Ok(())
}

// Admin-only. Re-pins the pinned USDC mint in the config. Intended for
// devnet flips from a throwaway test mint to Circle's canonical USDC. Safe
// ONLY when no tasks are currently funded on-chain — otherwise old task
// vaults become unreachable via approve_claim (they hold the old mint).
// The new fee_vault ATA must be created out-of-band before fees can land.
#[derive(Accounts)]
pub struct UpdateMint<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = admin @ EscrowError::Unauthorized,
    )]
    pub config: Account<'info, EscrowConfig>,

    pub new_mint: Account<'info, Mint>,
    pub admin: Signer<'info>,
}

pub fn update_mint(ctx: Context<UpdateMint>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    msg!(
        "USDC mint re-pinned from {} to {}",
        config.usdc_mint,
        ctx.accounts.new_mint.key()
    );
    config.usdc_mint = ctx.accounts.new_mint.key();
    Ok(())
}
