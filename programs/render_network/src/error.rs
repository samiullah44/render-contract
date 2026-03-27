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
    #[msg("Insufficient balance in escrow")]
    InsufficientEscrowBalance,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Arithmetic underflow")]
    Underflow,
    #[msg("No funds available to refund")]
    NoFundsToRefund,
    #[msg("Mint mismatch")]
    MintMismatch,
    #[msg("Job not yet finished or refunded")]
    JobNotFinished,
    #[msg("Release delay period has not yet passed")]
    ReleaseDelayNotMet,
}
