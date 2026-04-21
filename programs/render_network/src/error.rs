use anchor_lang::prelude::*;

#[error_code]
pub enum NetworkError {
    InvalidAmount,
    Unauthorized,
    JobIdMismatch,
    InvalidStatus,
    InsufficientFunds,
    Overflow,
    Underflow,
    NoFundsToRefund,
    MintMismatch,
    ReplayProtection,
    InsufficientWithdrawableBalance,
}
