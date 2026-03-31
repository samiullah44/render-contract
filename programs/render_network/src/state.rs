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
    pub locked_amount: u64,     // 8 bytes
    pub last_job_nonce: u64,    // 8 bytes
    pub bump: u8,               // 1 byte
    pub version: u8,            // 1 byte
}

/// Global configuration account
#[account]
#[derive(InitSpace)]
pub struct GlobalConfig {
    pub admin: Pubkey,
    pub fee_collector: Pubkey,
    pub platform_fee_bps: u64,
    pub version: u8,
}

pub const CONFIG_SEED: &[u8] = b"config_v3"; // BUMPED VERSION
pub const USER_ACCOUNT_SEED: &[u8] = b"user_account";

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
