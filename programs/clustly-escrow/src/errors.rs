use anchor_lang::prelude::*;

#[error_code]
pub enum EscrowError {
    #[msg("Fee bps exceeds hard cap (500 = 5%)")]
    FeeTooHigh,
    #[msg("Signer is not authorized")]
    Unauthorized,
    #[msg("Poster cannot enroll in own task")]
    SelfEnrollment,
    #[msg("All slots already filled")]
    SlotsFull,
    #[msg("Invalid state transition")]
    InvalidStateTransition,
    #[msg("Claim does not belong to task")]
    TaskClaimMismatch,
    #[msg("Task vault mint does not match configured USDC mint")]
    InvalidMint,
    #[msg("Bounty must be greater than zero")]
    ZeroBounty,
    #[msg("max_slots must be between 1 and 100")]
    InvalidSlots,
    #[msg("Deadline has not been reached")]
    DeadlineNotReached,
    #[msg("Not enough time has elapsed for auto action")]
    AutoActionTooEarly,
    #[msg("Enrolled claims still exist")]
    ClaimsNotResolved,
    #[msg("Rejection limit reached, claim already cancelled")]
    RejectionLimitReached,
    #[msg("Withdraw amount exceeds fee vault balance")]
    InsufficientFeeVault,
    #[msg("Task already has enrollments, cannot cancel")]
    TaskHasEnrollments,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Claim already submitted")]
    AlreadySubmitted,
}
