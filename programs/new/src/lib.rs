use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenInterface, TokenAccount, Mint, TransferChecked};
use anchor_spl::associated_token::AssociatedToken;

declare_id!("6SFP2GgBKFdakXKMZz4PrV8nyWnaMsS5mSN2YehApCgp");

#[program]
pub mod render_network {
    use super::*;

    /// Locks tokens from user into a job-specific escrow account
    pub fn lock_payment(
        ctx: Context<LockPayment>,
        job_id: u64,
        amount: u64,
    ) -> Result<()> {
        let escrow_key = ctx.accounts.escrow.key();
        {
            let escrow = &mut ctx.accounts.escrow;
            
            // Initialize escrow state
            escrow.job_id = job_id;
            escrow.user = ctx.accounts.user.key();
            escrow.mint = ctx.accounts.mint.key();
            escrow.amount = amount;
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
        
        msg!("Tokens successfully locked in escrow PDA: {}", escrow_key);
        Ok(())
    }

    /// Placeholder for Batch Release (Phase 3)
    pub fn batch_release(_ctx: Context<Placeholder>) -> Result<()> {
        Ok(())
    }

    /// Placeholder for Refund (Phase 4)
    pub fn refund_remaining(_ctx: Context<Placeholder>) -> Result<()> {
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
    pub released_amount: u64,  // 8 bytes
    pub status: u8,            // 1 byte
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

/// Placeholder Context
#[derive(Accounts)]
pub struct Placeholder {}