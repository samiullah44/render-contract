use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenInterface, TokenAccount, Mint, TransferChecked};
use anchor_spl::associated_token::AssociatedToken;

declare_id!("DWtobtz9kRZkCwh6s4FcN7yk6177rCY1T7xQHVdybmCz");

#[program]
pub mod render_network {
    use super::*;

    /// Initializes global configuration
    pub fn initialize_config(ctx: Context<InitializeConfig>, admin: Pubkey) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = admin;
        config.bump = ctx.bumps.config;
        Ok(())
    }

    /// Locks tokens from user into a job-specific escrow account
    pub fn lock_payment(
        ctx: Context<LockPayment>,
        job_id: u64,
        amount: u64,
    ) -> Result<()> {
        require!(amount > 0, NetworkError::InvalidAmount);
        
        let escrow_key = ctx.accounts.escrow.key();
        {
            let escrow = &mut ctx.accounts.escrow;
            
            // Initialize escrow state
            escrow.job_id = job_id;
            escrow.user = ctx.accounts.user.key();
            escrow.mint = ctx.accounts.mint.key();
            escrow.amount = amount;
            escrow.remaining_amount = amount; // Initial remaining is full amount
            escrow.released_amount = 0;
            escrow.status = EscrowStatus::Locked as u8;
            escrow.bump = ctx.bumps.escrow;
            
            msg!("Job ID: {}", job_id);
            msg!("Locking {} tokens for job {}", amount, job_id);
        }
        
        // Transfer tokens from user to escrow token account using TransferChecked for robustness
        token_interface::transfer_checked(
            ctx.accounts.transfer_ctx(),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        emit!(PaymentLocked {
            job_id,
            user: ctx.accounts.user.key(),
            mint: ctx.accounts.mint.key(),
            amount,
        });
        
        msg!("Tokens successfully locked in escrow PDA: {}", escrow_key);
        Ok(())
    }

    /// Placeholder for Batch Release (Phase 2)
    pub fn batch_release<'info>(ctx: Context<'_, '_, 'info, 'info, BatchRelease<'info>>, batch: Vec<BatchItem>) -> Result<()> {
        let config = &ctx.accounts.config;
        require!(ctx.accounts.admin.key() == config.admin, NetworkError::Unauthorized);

        let mut remaining_accounts_iter = ctx.remaining_accounts.iter();

        for item in batch.iter() {
            // 1. Get Escrow Account (Passed via remaining accounts)
            let escrow_info = next_account_info(&mut remaining_accounts_iter)?;
            let mut escrow = Account::<Escrow>::try_from(escrow_info)?;
            
            // 2. Get Escrow Token Account
            let escrow_token_info = next_account_info(&mut remaining_accounts_iter)?;
            let escrow_token_account = InterfaceAccount::<TokenAccount>::try_from(escrow_token_info)?;
            
            // Security: Verify Escrow Token Account ownership and mint
            require!(escrow_token_account.owner == escrow_info.key(), NetworkError::Unauthorized);
            require!(escrow_token_account.mint == escrow.mint, NetworkError::MintMismatch);
            require!(escrow.mint == ctx.accounts.mint.key(), NetworkError::MintMismatch);

            // Validate Escrow PDA and Job ID
            require!(escrow.job_id == item.job_id, NetworkError::JobIdMismatch);
            require!(escrow.status < EscrowStatus::Released as u8, NetworkError::InvalidStatus);

            let mut total_job_payout: u64 = 0;

            for payout in item.payouts.iter() {
                // 3. Get Provider Token Account
                let provider_token_info = next_account_info(&mut remaining_accounts_iter)?;
                let provider_token_account = InterfaceAccount::<TokenAccount>::try_from(provider_token_info)?;
                
                // Security: Verify Provider Token Account ownership and mint
                require!(provider_token_account.owner == payout.provider, NetworkError::Unauthorized);
                require!(provider_token_account.mint == escrow.mint, NetworkError::MintMismatch);

                // Math: Calculate total payout for this job in the batch
                total_job_payout = total_job_payout.checked_add(payout.amount).ok_or(NetworkError::Overflow)?;
                require!(total_job_payout <= escrow.remaining_amount, NetworkError::InsufficientEscrowBalance);

                // Perform Transfer
                let seeds = &[
                    b"escrow",
                    escrow.user.as_ref(),
                    &escrow.job_id.to_le_bytes(),
                    &[escrow.bump],
                ];
                let signer = &[&seeds[..]];

                token_interface::transfer_checked(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        TransferChecked {
                            from: escrow_token_info.to_account_info(),
                            to: provider_token_info.to_account_info(),
                            authority: escrow_info.to_account_info(),
                            mint: ctx.accounts.mint.to_account_info(),
                        },
                        signer,
                    ),
                    payout.amount,
                    ctx.accounts.mint.decimals,
                )?;

                emit!(PaymentReleased {
                    job_id: escrow.job_id,
                    provider: provider_token_info.key(),
                    amount: payout.amount,
                });
            }

            // Update Escrow State
            escrow.remaining_amount = escrow.remaining_amount.checked_sub(total_job_payout).ok_or(NetworkError::Underflow)?;
            escrow.released_amount = escrow.released_amount.checked_add(total_job_payout).ok_or(NetworkError::Overflow)?;
            
            if escrow.remaining_amount == 0 {
                escrow.status = EscrowStatus::Released as u8;
            } else {
                escrow.status = EscrowStatus::Partial as u8;
            }
            
            // Manually save the account because we loaded it from remaining_accounts
            escrow.exit(ctx.program_id)?;
        }

        Ok(())
    }

    /// Refunds remaining tokens to the user and marks job as Refunded (Phase 3)
    pub fn cancel_job(ctx: Context<CancelJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        require!(escrow.user == ctx.accounts.user.key(), NetworkError::Unauthorized);
        require!(escrow.status < EscrowStatus::Released as u8, NetworkError::InvalidStatus);

        let refund_amount = escrow.remaining_amount;
        require!(refund_amount > 0, NetworkError::NoFundsToRefund);

        // Transfer tokens back to user
        let seeds = &[
            b"escrow",
            escrow.user.as_ref(),
            &escrow.job_id.to_le_bytes(),
            &[escrow.bump],
        ];
        let signer = &[&seeds[..]];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.escrow_token_account.to_account_info(),
                    to: ctx.accounts.user_token_account.to_account_info(),
                    authority: escrow.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                },
                signer,
            ),
            refund_amount,
            ctx.accounts.mint.decimals,
        )?;

        // Update state
        escrow.remaining_amount = 0;
        escrow.status = EscrowStatus::Refunded as u8;

        emit!(JobCancelled {
            job_id: escrow.job_id,
            user: escrow.user,
            refund_amount,
        });

        Ok(())
    }
}

/// Escrow account storing state of a locked payment for a specific job
#[account]
#[derive(InitSpace)]
pub struct Escrow {
    pub job_id: u64,           // 8 bytes
    pub user: Pubkey,          // 32 bytes
    pub mint: Pubkey,          // 32 bytes
    pub amount: u64,           // 8 bytes
    pub remaining_amount: u64,  // 8 bytes
    pub released_amount: u64,  // 8 bytes
    pub status: u8,            // 1 byte
    pub bump: u8,              // 1 byte
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

/// Global configuration account
#[account]
#[derive(InitSpace)]
pub struct GlobalConfig {
    pub admin: Pubkey,         // 32 bytes
    pub bump: u8,              // 1 byte
}

/// Escrow status enumeration
#[repr(u8)]
pub enum EscrowStatus {
    Locked = 0,
    Partial = 1,
    Released = 2,
    Refunded = 3,
}

/// Accounts for initializing global config
#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + GlobalConfig::INIT_SPACE,
        seeds = [b"config"],
        bump
    )]
    pub config: Account<'info, GlobalConfig>,
    
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

/// Accounts required for locking tokens into escrow
#[derive(Accounts)]
#[instruction(job_id: u64, amount: u64)]
pub struct LockPayment<'info> {
    /// PDA that stores escrow state
    /// Seeds: ["escrow", user.key(), job_id]
    #[account(
        init,
        payer = user,
        space = 8 + Escrow::INIT_SPACE,
        seeds = [b"escrow", user.key().as_ref(), job_id.to_le_bytes().as_ref()],
        bump
    )]
    pub escrow: Account<'info, Escrow>,
    
    /// User locking the tokens
    #[account(mut)]
    pub user: Signer<'info>,
    
    /// The mint of the tokens being locked (Supports Token & Token-2022)
    pub mint: InterfaceAccount<'info, Mint>,
    
    /// User's token account
    #[account(
        mut,
        token::mint = mint,
        token::authority = user,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,
    
    /// Escrow's token account (ATA of the PDA)
    #[account(
        init,
        payer = user,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub escrow_token_account: InterfaceAccount<'info, TokenAccount>,
    
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CancelJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", user.key().as_ref(), escrow.job_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        has_one = user,
        has_one = mint,
    )]
    pub escrow: Account<'info, Escrow>,
    
    #[account(mut)]
    pub user: Signer<'info>,
    
    pub mint: InterfaceAccount<'info, Mint>,
    
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,
    
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub escrow_token_account: InterfaceAccount<'info, TokenAccount>,
    
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct BatchRelease<'info> {
    #[account(
        seeds = [b"config"],
        bump = config.bump
    )]
    pub config: Account<'info, GlobalConfig>,
    
    #[account(mut)]
    pub admin: Signer<'info>,
    
    pub mint: InterfaceAccount<'info, Mint>,
    
    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> LockPayment<'info> {
    pub fn transfer_ctx(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            TransferChecked {
                from: self.user_token_account.to_account_info(),
                to: self.escrow_token_account.to_account_info(),
                authority: self.user.to_account_info(),
                mint: self.mint.to_account_info(),
            },
        )
    }
}

/// Event: Payment locked
#[event]
pub struct PaymentLocked {
    pub job_id: u64,
    pub user: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
}

/// Event: Payment released
#[event]
pub struct PaymentReleased {
    pub job_id: u64,
    pub provider: Pubkey,
    pub amount: u64,
}

/// Event: Job cancelled
#[event]
pub struct JobCancelled {
    pub job_id: u64,
    pub user: Pubkey,
    pub refund_amount: u64,
}

/// Custom error codes
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
}

/// Placeholder Context
#[derive(Accounts)]
pub struct Placeholder {}