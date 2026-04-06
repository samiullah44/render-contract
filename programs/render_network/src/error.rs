use anchor_lang::prelude::*;

#[error_code]
pub enum NetworkError {
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("Unauthorized access")]
    Unauthorized,
    #[msg("Job ID mismatch")]
    JobIdMismatch,
    #[msg("Invalid escrow status")]
    InvalidStatus,
    #[msg("Insufficient balance in user account")]
    InsufficientFunds,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Arithmetic underflow")]
    Underflow,
    #[msg("No funds available to refund")]
    NoFundsToRefund,
    #[msg("Mint mismatch")]
    MintMismatch,
    #[msg("Replay protection: Job ID must be monotonically increasing")]
    ReplayProtection,
    #[msg("Insufficient withdrawable balance")]
    InsufficientWithdrawableBalance,
}
