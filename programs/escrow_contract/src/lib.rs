use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

// Program ID - Update this after deployment
declare_id!("FXH46myaENQa1cv2TN3vxAFTb3T2WGFqQi578ykeq9eW");

#[program]
pub mod escrow_contract {
    use super::*;

    /// Locks tokens from user into escrow account
    /// 
    /// # Arguments
    /// * `ctx` - Context containing accounts for the lock operation
    /// * `amount` - Amount of tokens to lock in escrow (in smallest units)
    /// 
    /// # Accounts
    /// * `escrow` - PDA that will store escrow state (initialized)
    /// * `user` - User locking the tokens (signer)
    /// * `provider` - Provider who will receive tokens upon release
    /// * `user_token_account` - User's SPL token account (source)
    /// * `escrow_token_account` - Escrow's SPL token account (destination)
    pub fn lock_payment(
        ctx: Context<LockPayment>,
        amount: u64,
    ) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        
        // Initialize escrow state
        escrow.user = *ctx.accounts.user.key;
        escrow.provider = *ctx.accounts.provider.key;
        escrow.amount = amount;
        escrow.status = EscrowStatus::Locked as u8;
        escrow.bump = ctx.bumps.escrow; // Store the PDA bump seed
        
        msg!("Locking {} tokens from user {}", amount, escrow.user);
        
        // Transfer tokens from user to escrow token account
        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.escrow_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        
        token::transfer(
            CpiContext::new(cpi_program, cpi_accounts),
            amount,
        )?;
        
        msg!("Tokens successfully locked in escrow");
        Ok(())
    }

    /// Releases tokens from escrow to the provider
    /// 
    /// # Arguments
    /// * `ctx` - Context containing accounts for the release operation
    /// 
    /// # Accounts
    /// * `escrow` - PDA storing escrow state (mutable)
    /// * `escrow_signer` - PDA signer for authorizing token transfer
    /// * `provider_token_account` - Provider's SPL token account (destination)
    /// * `escrow_token_account` - Escrow's SPL token account (source)
    pub fn release_payment(ctx: Context<ReleasePayment>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        
        // Validate escrow is still locked
        require!(
            escrow.status == EscrowStatus::Locked as u8,
            EscrowError::AlreadyReleased
        );
        
        msg!(
            "Releasing {} tokens to provider {}", 
            escrow.amount, 
            escrow.provider
        );
        
        // Transfer tokens from escrow to provider token account
        let cpi_accounts = Transfer {
            from: ctx.accounts.escrow_token_account.to_account_info(),
            to: ctx.accounts.provider_token_account.to_account_info(),
            authority: ctx.accounts.escrow_signer.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();

        // Create PDA signer seeds for authority
        let seeds = &[
            b"escrow".as_ref(),
            escrow.user.as_ref(),
            &[escrow.bump]
        ];
        let signer = &[&seeds[..]];
        
        token::transfer(
            CpiContext::new_with_signer(cpi_program, cpi_accounts, signer),
            escrow.amount,
        )?;
        
        // Update escrow status to released
        escrow.status = EscrowStatus::Released as u8;
        
        msg!("Tokens successfully released to provider");
        Ok(())
    }
}

/// Escrow account storing state of a locked payment
/// 
/// # Fields
/// * `user` - Public key of user who locked tokens
/// * `provider` - Public key of provider who receives tokens
/// * `amount` - Amount of tokens locked (in smallest units)
/// * `status` - Current status of escrow (0 = Locked, 1 = Released)
/// * `bump` - PDA bump seed for signing
#[account]
pub struct Escrow {
    pub user: Pubkey,
    pub provider: Pubkey,
    pub amount: u64,
    pub status: u8,
    pub bump: u8,
}

/// Escrow status enumeration for better code readability
#[repr(u8)]
pub enum EscrowStatus {
    Locked = 0,
    Released = 1,
}

/// Accounts required for locking tokens into escrow
/// 
/// Note: Using Box<> to allocate Escrow on heap and avoid stack overflow
#[derive(Accounts)]
pub struct LockPayment<'info> {
    /// PDA that stores escrow state
    /// 
    /// Seeds: ["escrow", user.key()]
    /// Space: 8 (discriminator) + 32 (user) + 32 (provider) + 8 (amount) + 1 (status) + 1 (bump)
    #[account(
        init,
        payer = user,
        space = 8 + 32 + 32 + 8 + 1 + 1,
        seeds = [b"escrow", user.key().as_ref()],
        bump
    )]
    pub escrow: Box<Account<'info, Escrow>>, // Box for heap allocation
    
    /// User locking the tokens (payer and signer)
    #[account(mut)]
    pub user: Signer<'info>,
    
    /// Provider who will receive tokens upon release
    /// 
    /// CHECK: Public key is provided off-chain, no validation needed
    #[account()]
    pub provider: UncheckedAccount<'info>,
    
    /// User's SPL token account (source of tokens)
    #[account(mut)]
    pub user_token_account: Account<'info, TokenAccount>,
    
    /// Escrow's SPL token account (destination for tokens)
    /// 
    /// Must be owned by the escrow PDA
    #[account(mut)]
    pub escrow_token_account: Account<'info, TokenAccount>,
    
    /// System program for account creation
    pub system_program: Program<'info, System>,
    
    /// SPL Token program for token transfers
    pub token_program: Program<'info, Token>,
    
    /// Rent sysvar for account initialization
    pub rent: Sysvar<'info, Rent>,
}

/// Accounts required for releasing tokens from escrow
/// 
/// Note: Using Box<> to allocate Escrow on heap and avoid stack overflow
#[derive(Accounts)]
pub struct ReleasePayment<'info> {
    /// PDA storing escrow state (mutable as status will be updated)
    /// 
    /// Seeds: ["escrow", escrow.user]
    #[account(
        mut,
        seeds = [b"escrow", escrow.user.as_ref()],
        bump = escrow.bump
    )]
    pub escrow: Box<Account<'info, Escrow>>, // Box for heap allocation
    
    /// PDA signer for authorizing token transfer from escrow
    /// 
    /// CHECK: This is the PDA that owns the escrow_token_account
    #[account(
        seeds = [b"escrow", escrow.user.as_ref()],
        bump = escrow.bump
    )]
    pub escrow_signer: UncheckedAccount<'info>,
    
    /// Provider's SPL token account (destination for released tokens)
    #[account(mut)]
    pub provider_token_account: Account<'info, TokenAccount>,
    
    /// Escrow's SPL token account (source of tokens for release)
    #[account(mut)]
    pub escrow_token_account: Account<'info, TokenAccount>,
    
    /// SPL Token program for token transfers
    pub token_program: Program<'info, Token>,
}

/// Custom error codes for the escrow program
#[error_code]
pub enum EscrowError {
    /// Payment has already been released from escrow
    #[msg("Payment already released")]
    AlreadyReleased,
    
    /// Invalid escrow state
    #[msg("Invalid escrow state")]
    InvalidState,
}