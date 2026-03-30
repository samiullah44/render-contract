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

    // ─────────────────────────────────────────────────────────────
    // ADMIN: Global Config
    // ─────────────────────────────────────────────────────────────

    pub fn initialize_config(ctx: Context<InitializeConfig>, admin: Pubkey, release_delay: i64) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = admin;
        config.release_delay = release_delay;
        config.bump = ctx.bumps.config;
        msg!("Global config initialized. Admin: {}", admin);
        Ok(())
    }

    pub fn update_config(ctx: Context<UpdateConfig>, new_admin: Option<Pubkey>, new_delay: Option<i64>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        if let Some(admin) = new_admin { config.admin = admin; }
        if let Some(delay) = new_delay { config.release_delay = delay; }
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // USER: Deposit tokens to their "Website Credit Account"
    // Account is initialized on first deposit (user pays rent).
    // Gas fee is paid by the user as part of the transaction.
    // ─────────────────────────────────────────────────────────────


    pub fn deposit_to_account(ctx: Context<DepositToAccount>, user_id: Pubkey, amount: u64) -> Result<()> {

        require!(amount > 0, NetworkError::InvalidAmount);

        let user_account = &mut ctx.accounts.user_account;

        // Initialize on first deposit
        if user_account.owner == Pubkey::default() {
            user_account.owner   = ctx.accounts.user.key();
            user_account.user_id = user_id;
            user_account.mint    = ctx.accounts.mint.key();
            user_account.credited_amount = 0;
            user_account.bump    = ctx.bumps.user_account;
        }


        // Verify mint matches
        require!(user_account.mint == ctx.accounts.mint.key(), NetworkError::MintMismatch);

        // Transfer: User Wallet ATA → User Credit PDA ATA
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

        msg!("Deposited {} tokens to credit account. New balance: {}", amount, user_account.credited_amount);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // USER: Lock payment from Credit Account → Escrow PDA (Job Start)
    // ─────────────────────────────────────────────────────────────


    pub fn lock_payment(ctx: Context<LockPayment>, user_id: Pubkey, job_id: u64, amount: u64) -> Result<()> {

        require!(amount > 0, NetworkError::InvalidAmount);

        // Verify sufficient credits
        let user_account = &mut ctx.accounts.user_deposit_account;
        require!(user_account.credited_amount >= amount, NetworkError::InsufficientEscrowBalance);
        require!(user_account.mint == ctx.accounts.mint.key(), NetworkError::MintMismatch);

        // Initialize Escrow
        let escrow = &mut ctx.accounts.escrow;
        escrow.job_id           = job_id;
        escrow.user_id          = user_id;
        escrow.user             = ctx.accounts.user.key();
        escrow.mint             = ctx.accounts.mint.key();
        escrow.amount           = amount;
        escrow.remaining_amount = amount;
        escrow.released_amount  = 0;
        escrow.completed_at     = 0;
        escrow.status           = EscrowStatus::Locked as u8;
        escrow.bump             = ctx.bumps.escrow;


        // Transfer: User Credit PDA ATA → Escrow PDA ATA
        // Signed by the UserAccount PDA
        let bump = user_account.bump;

        let seeds = &[
            b"user_account",
            user_account.user_id.as_ref(), // Use the stored user_id for signing
            &[bump],
        ];

        let signer = &[&seeds[..]];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from:      ctx.accounts.user_deposit_token_account.to_account_info(),
                    to:        ctx.accounts.escrow_token_account.to_account_info(),
                    authority: user_account.to_account_info(),
                    mint:      ctx.accounts.mint.to_account_info(),
                },
                signer,
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        // Deduct from credits
        user_account.credited_amount = user_account.credited_amount
            .checked_sub(amount).ok_or(NetworkError::Underflow)?;

        emit!(PaymentLocked { job_id, user: ctx.accounts.user.key(), mint: ctx.accounts.mint.key(), amount });
        msg!("Locked {} tokens from Credits into Escrow. Remaining credits: {}", amount, user_account.credited_amount);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // ADMIN: Mark job completed (starts release timer)
    // ─────────────────────────────────────────────────────────────

    pub fn mark_job_completed(ctx: Context<MarkJobCompleted>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        let config = &ctx.accounts.config;

        require!(ctx.accounts.admin.key() == config.admin, NetworkError::Unauthorized);
        require!(escrow.completed_at == 0, NetworkError::InvalidStatus);

        escrow.completed_at = Clock::get()?.unix_timestamp;
        msg!("Job {} marked completed. Release delay timer started.", escrow.job_id);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // ADMIN: Batch Release → Node Providers (after release delay)
    // ─────────────────────────────────────────────────────────────

    pub fn batch_release<'info>(ctx: Context<'_, '_, 'info, 'info, BatchRelease<'info>>, batch: Vec<BatchItem>) -> Result<()> {
        let config = &ctx.accounts.config;
        require!(ctx.accounts.admin.key() == config.admin, NetworkError::Unauthorized);

        let mut remaining_accounts_iter = ctx.remaining_accounts.iter();

        for item in batch.iter() {
            let escrow_info       = next_account_info(&mut remaining_accounts_iter)?;
            let mut escrow        = Account::<Escrow>::try_from(escrow_info)?;
            let escrow_token_info = next_account_info(&mut remaining_accounts_iter)?;
            let escrow_token_account = InterfaceAccount::<TokenAccount>::try_from(escrow_token_info)?;

            // Security checks
            require!(escrow_token_account.owner == escrow_info.key(), NetworkError::Unauthorized);
            require!(escrow_token_account.mint  == escrow.mint, NetworkError::MintMismatch);
            require!(escrow.mint == ctx.accounts.mint.key(), NetworkError::MintMismatch);
            require!(escrow.job_id == item.job_id, NetworkError::JobIdMismatch);
            require!(escrow.status < EscrowStatus::Released as u8, NetworkError::InvalidStatus);
            require!(escrow.completed_at > 0, NetworkError::JobNotFinished);
            let current_time = Clock::get()?.unix_timestamp;
            require!(current_time >= escrow.completed_at + config.release_delay, NetworkError::ReleaseDelayNotMet);

            let mut total_job_payout: u64 = 0;

            for payout in item.payouts.iter() {
                let provider_token_info    = next_account_info(&mut remaining_accounts_iter)?;
                let provider_token_account = InterfaceAccount::<TokenAccount>::try_from(provider_token_info)?;

                require!(provider_token_account.owner == payout.provider, NetworkError::Unauthorized);
                require!(provider_token_account.mint  == escrow.mint, NetworkError::MintMismatch);

                total_job_payout = total_job_payout.checked_add(payout.amount).ok_or(NetworkError::Overflow)?;
                require!(total_job_payout <= escrow.remaining_amount, NetworkError::InsufficientEscrowBalance);

                let seeds  = &[b"escrow", escrow.user_id.as_ref(), &escrow.job_id.to_le_bytes(), &[escrow.bump]];

                let signer = &[&seeds[..]];

                token_interface::transfer_checked(
                    CpiContext::new_with_signer(
                        ctx.accounts.token_program.to_account_info(),
                        TransferChecked {
                            from:      escrow_token_info.to_account_info(),
                            to:        provider_token_info.to_account_info(),
                            authority: escrow_info.to_account_info(),
                            mint:      ctx.accounts.mint.to_account_info(),
                        },
                        signer,
                    ),
                    payout.amount,
                    ctx.accounts.mint.decimals,
                )?;

                emit!(PaymentReleased { job_id: escrow.job_id, provider: provider_token_info.key(), amount: payout.amount });
            }

            escrow.remaining_amount = escrow.remaining_amount.checked_sub(total_job_payout).ok_or(NetworkError::Underflow)?;
            escrow.released_amount  = escrow.released_amount.checked_add(total_job_payout).ok_or(NetworkError::Overflow)?;
            escrow.status = if escrow.remaining_amount == 0 { EscrowStatus::Released as u8 } else { EscrowStatus::Partial as u8 };

            escrow.exit(ctx.program_id)?;
        }

        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // USER: Cancel Job → Refund back to Credit Account
    // ─────────────────────────────────────────────────────────────

    pub fn cancel_job(ctx: Context<CancelJob>) -> Result<()> {
        let escrow = &mut ctx.accounts.escrow;
        require!(escrow.user == ctx.accounts.user.key(), NetworkError::Unauthorized);
        require!(escrow.status < EscrowStatus::Released as u8, NetworkError::InvalidStatus);

        let refund_amount = escrow.remaining_amount;
        require!(refund_amount > 0, NetworkError::NoFundsToRefund);

        let seeds  = &[b"escrow", escrow.user_id.as_ref(), &escrow.job_id.to_le_bytes(), &[escrow.bump]];
        let signer = &[&seeds[..]];

        // Transfer: Escrow PDA ATA → User Credit PDA ATA
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from:      ctx.accounts.escrow_token_account.to_account_info(),
                    to:        ctx.accounts.user_deposit_token_account.to_account_info(),
                    authority: escrow.to_account_info(),
                    mint:      ctx.accounts.mint.to_account_info(),
                },
                signer,
            ),
            refund_amount,
            ctx.accounts.mint.decimals,
        )?;

        // Close the Escrow Token Account (reclaim rent → user)
        token_interface::close_account(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token_interface::CloseAccount {
                    account:     ctx.accounts.escrow_token_account.to_account_info(),
                    destination: ctx.accounts.user.to_account_info(),
                    authority:   escrow.to_account_info(),
                },
                signer,
            ),
        )?;

        // Restore credits
        ctx.accounts.user_deposit_account.credited_amount = ctx.accounts.user_deposit_account.credited_amount
            .checked_add(refund_amount).ok_or(NetworkError::Overflow)?;

        escrow.remaining_amount = 0;
        escrow.status = EscrowStatus::Refunded as u8;

        emit!(JobCancelled { job_id: escrow.job_id, user: escrow.user, refund_amount });
        msg!("Job {} cancelled. {} tokens refunded to credit account.", escrow.job_id, refund_amount);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // USER: Close fully-released Escrow (reclaim rent)
    // ─────────────────────────────────────────────────────────────

    pub fn close_escrow(ctx: Context<CloseEscrow>) -> Result<()> {
        let escrow = &ctx.accounts.escrow;
        let seeds  = &[b"escrow", escrow.user_id.as_ref(), &escrow.job_id.to_le_bytes(), &[escrow.bump]];

        let signer = &[&seeds[..]];

        token_interface::close_account(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token_interface::CloseAccount {
                    account:     ctx.accounts.escrow_token_account.to_account_info(),
                    destination: ctx.accounts.user.to_account_info(),
                    authority:   escrow.to_account_info(),
                },
                signer,
            ),
        )?;

        msg!("Escrow closed. Rent returned to user.");
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ACCOUNT CONTEXTS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + GlobalConfig::INIT_SPACE,
        seeds = [b"config_v2"],
        bump
    )]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    #[account(
        mut,
        seeds = [b"config_v2"],
        bump = config.bump,
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

/// User deposits tokens from their wallet into their Credit PDA.
/// On first call: creates the UserAccount PDA and its ATA.
/// User pays all rent (no platform cost).
#[derive(Accounts)]
#[instruction(user_id: Pubkey, amount: u64)]
pub struct DepositToAccount<'info> {

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + UserAccount::INIT_SPACE,
        seeds = [b"user_account", user_id.as_ref()],
        bump
    )]
    pub user_account: Box<Account<'info, UserAccount>>,


    #[account(mut)]
    pub user: Signer<'info>,

    pub mint: Box<InterfaceAccount<'info, Mint>>,

    /// User's own wallet token account (source of funds)
    #[account(
        mut,
        token::mint = mint,
        token::authority = user,
    )]
    pub user_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Credit PDA's token account (destination). Initialized if needed.
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

/// User starts a job: moves tokens from their Credit Account → Job Escrow.
#[derive(Accounts)]
#[instruction(user_id: Pubkey, job_id: u64, amount: u64)]
pub struct LockPayment<'info> {
    #[account(
        init,
        payer = user,
        space = 8 + Escrow::INIT_SPACE,
        seeds = [b"escrow", user_id.as_ref(), job_id.to_le_bytes().as_ref()],
        bump
    )]
    pub escrow: Box<Account<'info, Escrow>>,

    #[account(mut)]
    pub user: Signer<'info>,

    /// The user's Credit Account (source of funds)
    #[account(
        mut,
        seeds = [b"user_account", user_id.as_ref()],
        bump = user_deposit_account.bump,
    )]
    pub user_deposit_account: Box<Account<'info, UserAccount>>,


    pub mint: Box<InterfaceAccount<'info, Mint>>,

    /// Credit PDA's token ATA (source of tokens)
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user_deposit_account,
    )]
    pub user_deposit_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Escrow PDA's token ATA (destination). Created here.
    #[account(
        init,
        payer = user,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub escrow_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct MarkJobCompleted<'info> {
    #[account(seeds = [b"config_v2"], bump = config.bump)]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(
        mut,
        seeds = [b"escrow", escrow.user_id.as_ref(), escrow.job_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
    )]
    pub escrow: Account<'info, Escrow>,

}

#[derive(Accounts)]
pub struct BatchRelease<'info> {
    #[account(seeds = [b"config_v2"], bump = config.bump)]
    pub config: Account<'info, GlobalConfig>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub mint: InterfaceAccount<'info, Mint>,
    pub token_program: Interface<'info, TokenInterface>,
}

/// Cancel job: Escrow tokens → User Credit Account. Escrow ATA closed.
#[derive(Accounts)]
pub struct CancelJob<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow.user_id.as_ref(), escrow.job_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        has_one = mint,
        close = user,
    )]
    pub escrow: Box<Account<'info, Escrow>>,


    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"user_account", escrow.user_id.as_ref()],
        bump = user_deposit_account.bump,
    )]
    pub user_deposit_account: Box<Account<'info, UserAccount>>,


    pub mint: Box<InterfaceAccount<'info, Mint>>,

    /// Refund destination: User's Credit PDA ATA
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = user_deposit_account,
    )]
    pub user_deposit_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Escrow's token ATA (to be closed)
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub escrow_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Close a fully-released escrow to reclaim rent.
#[derive(Accounts)]
pub struct CloseEscrow<'info> {
    #[account(
        mut,
        seeds = [b"escrow", escrow.user_id.as_ref(), escrow.job_id.to_le_bytes().as_ref()],
        bump = escrow.bump,
        has_one = mint,
        constraint = escrow.remaining_amount == 0 @ NetworkError::JobNotFinished,
        close = user,
    )]
    pub escrow: Account<'info, Escrow>,


    #[account(mut)]
    pub user: Signer<'info>,

    pub mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub escrow_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

// ─────────────────────────────────────────────────────────────────────────────
// EVENTS
// ─────────────────────────────────────────────────────────────────────────────

#[event]
pub struct PaymentLocked {
    pub job_id: u64,
    pub user: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
}

#[event]
pub struct PaymentReleased {
    pub job_id: u64,
    pub provider: Pubkey,
    pub amount: u64,
}

#[event]
pub struct JobCancelled {
    pub job_id: u64,
    pub user: Pubkey,
    pub refund_amount: u64,
}