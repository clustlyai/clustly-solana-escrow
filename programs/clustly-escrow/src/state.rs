use anchor_lang::prelude::*;

pub const FEE_BPS_CAP: u16 = 500; // 5% hard cap
pub const DEFAULT_FEE_BPS: u16 = 400; // 4%
pub const BPS_DENOMINATOR: u64 = 10_000;

pub const AUTO_APPROVE_WINDOW_SECS: i64 = 14 * 24 * 60 * 60; // 14 days
pub const AUTO_DISPUTE_WINDOW_SECS: i64 = 14 * 24 * 60 * 60; // 14 days
pub const MAX_REJECTIONS: u8 = 3;
pub const MAX_SLOTS_LIMIT: u16 = 100;

pub const CONFIG_SEED: &[u8] = b"config";
pub const TASK_SEED: &[u8] = b"task";
pub const CLAIM_SEED: &[u8] = b"claim";
pub const TASK_VAULT_SEED: &[u8] = b"vault";
pub const FEE_VAULT_SEED: &[u8] = b"fee_vault";

#[account]
pub struct EscrowConfig {
    pub admin: Pubkey,
    pub usdc_mint: Pubkey,
    pub fee_bps: u16,
    pub bump: u8,
    pub fee_vault_bump: u8,
    pub _reserved: [u8; 64],
}

impl EscrowConfig {
    pub const SIZE: usize = 32 + 32 + 2 + 1 + 1 + 64;
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskStatus {
    Open = 0,
    Closed = 1,
}

#[account]
pub struct EscrowTask {
    pub task_id: [u8; 16],
    pub poster: Pubkey,
    pub bounty: u64, // per-slot, in micro-USDC (6 decimals)
    pub max_slots: u16,
    pub slots_enrolled: u16,
    pub slots_approved: u16,
    pub slots_refunded: u16, // slots refunded from rejections @ MAX_REJECTIONS
    pub status: u8,          // TaskStatus
    pub task_deadline_ts: i64,
    pub claim_deadline_secs: u32,
    pub bump: u8,
    pub vault_bump: u8,
    pub _reserved: [u8; 32],
}

impl EscrowTask {
    pub const SIZE: usize = 16 + 32 + 8 + 2 + 2 + 2 + 2 + 1 + 8 + 4 + 1 + 1 + 32;

    pub fn status(&self) -> TaskStatus {
        match self.status {
            0 => TaskStatus::Open,
            _ => TaskStatus::Closed,
        }
    }

    pub fn set_status(&mut self, s: TaskStatus) {
        self.status = s as u8;
    }
}

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClaimStatus {
    Enrolled = 0,
    Submitted = 1,
    Approved = 2,
    Rejected = 3,
    Cancelled = 4,
    Disputed = 5,
    ResolvedDeliverer = 6,
    ResolvedPoster = 7,
}

impl ClaimStatus {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ClaimStatus::Enrolled),
            1 => Some(ClaimStatus::Submitted),
            2 => Some(ClaimStatus::Approved),
            3 => Some(ClaimStatus::Rejected),
            4 => Some(ClaimStatus::Cancelled),
            5 => Some(ClaimStatus::Disputed),
            6 => Some(ClaimStatus::ResolvedDeliverer),
            7 => Some(ClaimStatus::ResolvedPoster),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ClaimStatus::Approved
                | ClaimStatus::Cancelled
                | ClaimStatus::ResolvedDeliverer
                | ClaimStatus::ResolvedPoster
        )
    }
}

#[account]
pub struct EscrowClaim {
    pub claim_id: [u8; 16],
    pub task: Pubkey,
    pub claimer: Pubkey,
    pub status: u8, // ClaimStatus
    pub enrolled_at: i64,
    pub submitted_at: i64,
    pub disputed_at: i64,
    pub rejections: u8,
    pub deliverable_hash: [u8; 32],
    pub bump: u8,
    pub _reserved: [u8; 16],
}

impl EscrowClaim {
    pub const SIZE: usize = 16 + 32 + 32 + 1 + 8 + 8 + 8 + 1 + 32 + 1 + 16;

    pub fn status(&self) -> Result<ClaimStatus> {
        ClaimStatus::from_u8(self.status).ok_or_else(|| error!(crate::errors::EscrowError::InvalidStateTransition))
    }

    pub fn set_status(&mut self, s: ClaimStatus) {
        self.status = s as u8;
    }
}

pub fn compute_fee_split(bounty: u64, fee_bps: u16) -> Result<(u64, u64)> {
    let fee = (bounty as u128)
        .checked_mul(fee_bps as u128)
        .ok_or(crate::errors::EscrowError::MathOverflow)?
        .checked_div(BPS_DENOMINATOR as u128)
        .ok_or(crate::errors::EscrowError::MathOverflow)? as u64;
    let net = bounty.checked_sub(fee).ok_or(crate::errors::EscrowError::MathOverflow)?;
    Ok((net, fee))
}
