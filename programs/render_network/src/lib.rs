use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenInterface, TokenAccount, Mint, TransferChecked};
use anchor_spl::associated_token::AssociatedToken;

mod state;
mod error;

use crate::state::*;
use crate::error::*;

declare_id!("DWtobtz9kRZkCwh6s4FcN7yk6177rCY1T7xQHVdybmCz");

#[program]
pub mod render_network {
    use super::*;

    pub fn initialize_global_config(ctx: Context<InitializeGlobalConfig>, platform_fee_bps: u64) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.fee_collector = ctx.accounts.fee_collector.key();
        config.platform_fee_bps = platform_fee_bps;
        config.version = 1;
        Ok(())
    }

    pub fn update_global_config(
        ctx: Context<UpdateGlobalConfig>,
        new_admin: Option<Pubkey>,
        new_fee_collector: Option<Pubkey>,
        new_platform_fee_bps: Option<u64>
    ) -> Result<()> {
        let config = &mut ctx.accounts.config;
        if let Some(admin) = new_admin { config.admin = admin; }
        if let Some(collector) = new_fee_collector { config.fee_collector = collector; }
        if let Some(bps) = new_platform_fee_bps { config.platform_fee_bps = bps; }
        Ok(())
    }

    pub fn deposit_to_account(ctx: Context<DepositToAccount>, user_id: Pubkey, amount: u64) -> Result<()> {
        require!(amount > 0, NetworkError::InvalidAmount);
        let user_account = &mut ctx.accounts.user_account;

        if user_account.owner == Pubkey::default() {
            user_account.owner   = ctx.accounts.user.key();
            user_account.user_id = user_id;
            user_account.mint    = ctx.accounts.mint.key();
            user_account.credited_amount = 0;
            user_account.locked_amount = 0;
            user_account.last_job_nonce = 0;
            user_account.version = 1;
            user_account.bump    = ctx.bumps.user_account;
        }

        require!(user_account.mint == ctx.accounts.mint.key(), NetworkError::MintMismatch);

        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from:      ctx.accounts.user_token_account.to_account_info(),
                    to:        ctx.accounts.user_deposit_token_account.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                    mint:      ctx.accounts.mint.to_account_info(),
                },
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        user_account.credited_amount = user_account.credited_amount
            .checked_add(amount).ok_or(NetworkError::Overflow)?;

        Ok(())
    }

    pub fn admin_lock_payment(ctx: Context<AdminLockPayment>, job_id: u64, amount: u64) -> Result<()> {
        require!(amount > 0, NetworkError::InvalidAmount);
        let user_account = &mut ctx.accounts.user_account;
        
        require!(job_id > user_account.last_job_nonce, NetworkError::ReplayProtection);
        let available_balance = user_account.credited_amount.checked_sub(user_account.locked_amount).ok_or(NetworkError::Underflow)?;
        require!(available_balance >= amount, NetworkError::InsufficientFunds);
        
        user_account.locked_amount = user_account.locked_amount.checked_add(amount).ok_or(NetworkError::Overflow)?;
        user_account.last_job_nonce = job_id;
        
        emit!(JobLocked {
            job_id,
            user: user_account.owner,
            amount
        });
        
        Ok(())
    }

    pub fn batch_release<'info>(ctx: Context<'_, '_, 'info, 'info, BatchRelease<'info>>, batch: Vec<BatchItem>) -> Result<()> {
        let config = &ctx.accounts.config;
        let user_account = &mut ctx.accounts.user_account;
        
        require!(ctx.accounts.admin.key() == config.admin, NetworkError::Unauthorized);

        let mut remaining_accounts_iter = ctx.remaining_accounts.iter();
        
        let user_id = user_account.user_id;
        let bump = user_account.bump;
        let seeds = &[
            USER_ACCOUNT_SEED,
            user_id.as_ref(),
            &[bump],
        ];
        let signer = &[&seeds[..]];

        for item in batch.iter() {
            let mut total_job_payout: u64 = 0;
            let mut total_fee: u64 = 0;

            for payout in item.payouts.iter() {
                let provider_token_info    = next_account_info(&mut remaining_accounts_iter)?;
                let provider_token_account = InterfaceAccount::<TokenAccount>::try_from(provider_token_info)?;

                require!(provider_token_account.owner == payout.provider, NetworkError::Unauthorized);
                require!(provider_token_account.mint  == ctx.accounts.mint.key(), NetworkError::MintMismatch);
                
                let fee = (payout.amount as u128)
                    .checked_mul(config.platform_fee_bps as u128)
                    .ok_or(NetworkError::Overflow)?
                    .checked_div(10000)
                    .ok_or(NetworkError::Underflow)? as u64;
                    
                let provider_amount = payout.amount.checked_sub(fee).ok_or(NetworkError::Underflow)?;

                total_job_payout = total_job_payout.checked_add(payout.amount).ok_or(NetworkError::Overflow)?;
                total_fee = total_fee.checked_add(fee).ok_or(NetworkError::Overflow)?;

                if provider_amount > 0 {
                    token_interface::transfer_checked(
                        CpiContext::new_with_signer(
                            ctx.accounts.token_program.to_account_info(),
                            TransferChecked {
                                from:      ctx.accounts.user_deposit_token_account.to_account_info(),
                                to:        provider_token_info.to_account_info(),
                                authority: user_account.to_account_info(),
                                mint:      ctx.accounts.mint.to_account_info(),
                            },
                            signer,
                        ),
                        provider_amount,
                        ctx.accounts.mint.decimals,
                    )?;
                }
                
                emit!(PaymentReleased {
                    job_id: item.job_id,
                    provider: payout.provider,
                    amount: provider_amount
                });
            }
            
            require!(total_job_payout <= user_account.locked_amount, NetworkError::InsufficientFunds);
            
            user_account.locked_amount = user_account.locked_amount.checked_sub(total_job_payout).ok_or(NetworkError::Underflow)?;
            user_account.credited_amount = user_account.credited_amount.checked_sub(total_job_payout).ok_or(NetworkError::Underflow)?;

            if total_fee > 0 {
                token_interface::transfer_checked(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        TransferChecked {
                            from:      ctx.accounts.user_deposit_token_account.to_account_info(),
                            to:        ctx.accounts.fee_collector_token_account.to_account_info(),
                            authority: user_account.to_account_info(),
                            mint:      ctx.accounts.mint.to_account_info(),
                        },
                        signer,
                    ),
                    total_fee,
                    ctx.accounts.mint.decimals,
                )?;
            }
            
            emit!(BatchPaid {
                job_id: item.job_id,
                user: user_account.owner,
                total_payout: total_job_payout,
                fee_collected: total_fee,
                timestamp: Clock::get()?.unix_timestamp
            });
        }

        Ok(())
    }

    pub fn admin_cancel_payment(ctx: Context<AdminLockPayment>, job_id: u64, amount: u64) -> Result<()> {
        require!(amount > 0, NetworkError::InvalidAmount);
        let user_account = &mut ctx.accounts.user_account;
        
        require!(user_account.locked_amount >= amount, NetworkError::Underflow);
        user_account.locked_amount = user_account.locked_amount.checked_sub(amount).ok_or(NetworkError::Underflow)?;
        
        emit!(JobUnlocked {
            job_id,
            user: user_account.owner,
            amount
        });
        
        Ok(())
    }

    pub fn withdraw_from_account(ctx: Context<WithdrawFromAccount>, amount: u64) -> Result<()> {
        require!(amount > 0, NetworkError::InvalidAmount);
        let user_account = &mut ctx.accounts.user_account;
        
        let available_balance = user_account.credited_amount.checked_sub(user_account.locked_amount).ok_or(NetworkError::Underflow)?;
        require!(available_balance >= amount, NetworkError::InsufficientWithdrawableBalance);
        
        let user_id = user_account.user_id;
        let bump = user_account.bump;
        let seeds = &[
            USER_ACCOUNT_SEED,
            user_id.as_ref(),
            &[bump],
        ];
        let signer = &[&seeds[..]];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from:      ctx.accounts.user_deposit_token_account.to_account_info(),
                    to:        ctx.accounts.user_token_account.to_account_info(),
                    authority: user_account.to_account_info(),
                    mint:      ctx.accounts.mint.to_account_info(),
                },
                signer,
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        user_account.credited_amount = user_account.credited_amount.checked_sub(amount).ok_or(NetworkError::Underflow)?;

        emit!(TokensWithdrawn {
            user: ctx.accounts.user.key(),
            amount
        });
        
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ACCOUNT CONTEXTS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitializeGlobalConfig<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + GlobalConfig::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    /// CHECK: This is just to record the address
    pub fee_collector: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateGlobalConfig<'info> {
    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump,
        realloc = 8 + GlobalConfig::INIT_SPACE,
        realloc::payer = admin,
        realloc::zero = false,
        constraint = config.admin == admin.key() @ NetworkError::Unauthorized,
    )]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(user_id: Pubkey, amount: u64)]
pub struct DepositToAccount<'info> {
    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserAccount::INIT_SPACE,
        seeds = [USER_ACCOUNT_SEED, user_id.as_ref()],
        bump,
    )]
    pub user_account: Box<Account<'info, UserAccount>>,
    #[account(mut)]
    pub user: Signer<'info>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = user,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint,
        associated_token::authority = user_account,
    )]
    pub user_deposit_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminLockPayment<'info> {
    #[account(
        seeds = [CONFIG_SEED],
        bump,
        has_one = admin
    )]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut, constraint = config.admin == admin.key() @ NetworkError::Unauthorized)]
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, user_account.user_id.as_ref()],
        bump = user_account.bump,
    )]
    pub user_account: Account<'info, UserAccount>,
}

#[derive(Accounts)]
pub struct BatchRelease<'info> {
    #[account(seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut, constraint = config.admin == admin.key() @ NetworkError::Unauthorized)]
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, user_account.user_id.as_ref()],
        bump = user_account.bump,
    )]
    pub user_account: Account<'info, UserAccount>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user_account,
    )]
    pub user_deposit_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = config.fee_collector,
    )]
    pub fee_collector_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct WithdrawFromAccount<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        seeds = [USER_ACCOUNT_SEED, user_account.user_id.as_ref()],
        bump = user_account.bump,
        constraint = user_account.owner == user.key() @ NetworkError::Unauthorized
    )]
    pub user_account: Account<'info, UserAccount>,
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint,
        associated_token::authority = user,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user_account,
    )]
    pub user_deposit_token_account: Box<InterfaceAccount<'info, TokenAccount>>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

// ─────────────────────────────────────────────────────────────────────────────
// EVENTS
// ─────────────────────────────────────────────────────────────────────────────

#[event]
pub struct JobUnlocked {
    pub job_id: u64,
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct JobLocked {
    pub job_id: u64,
    pub user: Pubkey,
    pub amount: u64,
}

#[event]
pub struct PaymentReleased {
    pub job_id: u64,
    pub provider: Pubkey,
    pub amount: u64,
}

#[event]
pub struct BatchPaid {
    pub job_id: u64,
    pub user: Pubkey,
    pub total_payout: u64,
    pub fee_collected: u64,
    pub timestamp: i64,
}

#[event]
pub struct TokensWithdrawn {
    pub user: Pubkey,
    pub amount: u64,
}