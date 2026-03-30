use anchor_lang::prelude::*;

/// Stores a user's "Website Balance" (Credits)
/// Created on first deposit. User pays rent.
#[account]
#[derive(InitSpace)]
pub struct UserAccount {
    pub owner: Pubkey,          // 32 bytes (The wallet that created/last used it)
    pub user_id: Pubkey,        // 32 bytes (The Website Identity Seed)
    pub mint: Pubkey,           // 32 bytes
    pub credited_amount: u64,   // 8 bytes
    pub bump: u8,               // 1 byte

}

/// Escrow account storing state of a locked payment for a specific job
#[account]
#[derive(InitSpace)]
pub struct Escrow {
    pub job_id: u64,            // 8 bytes
    pub user_id: Pubkey,        // 32 bytes (The Website Identity Seed)
    pub user: Pubkey,           // 32 bytes (The wallet that locked it)
    pub mint: Pubkey,           // 32 bytes
    pub amount: u64,            // 8 bytes
    pub remaining_amount: u64,  // 8 bytes
    pub released_amount: u64,   // 8 bytes
    pub completed_at: i64,      // 8 bytes (timestamp, 0 = not completed)
    pub status: u8,             // 1 byte
    pub bump: u8,               // 1 byte

}

/// Global configuration account
#[account]
#[derive(InitSpace)]
pub struct GlobalConfig {
    pub admin: Pubkey,          // 32 bytes
    pub release_delay: i64,     // 8 bytes (seconds)
    pub bump: u8,               // 1 byte
}

/// Escrow status enumeration
#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq)]
pub enum EscrowStatus {
    Locked = 0,
    Partial = 1,
    Released = 2,
    Refunded = 3,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Payout {
    pub provider: Pubkey,
    pub amount: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct BatchItem {
    pub job_id: u64,
    pub payouts: Vec<Payout>,
}
