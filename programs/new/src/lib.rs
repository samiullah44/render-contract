use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenInterface, TokenAccount, Mint, TransferChecked};
use std::str::FromStr;

declare_id!("6SFP2GgBKFdakXKMZz4PrV8nyWnaMsS5mSN2YehApCgp");

const RNDR_MINT: &str = "2PdTeB2ac5hf2V17Guq1Kb7YAU7mGeZPSnYgYAxvNPs1";

#[program]
pub mod render_network {
    use super::*;

    pub fn deposit_rndr(ctx: Context<Deposit>, amount: u64) -> Result<()> {
        require_keys_eq!(
            ctx.accounts.token_mint.key(),
            Pubkey::from_str(RNDR_MINT).unwrap(),
            CustomError::InvalidToken
        );

        token_interface::transfer_checked(
            ctx.accounts.transfer_ctx(),
            amount,
            ctx.accounts.token_mint.decimals,
        )?;

        ctx.accounts.user_account.balance += amount;
        Ok(())
    }

    pub fn withdraw_rndr(ctx: Context<Withdraw>, amount: u64) -> Result<()> {
        require_keys_eq!(
            ctx.accounts.token_mint.key(),
            Pubkey::from_str(RNDR_MINT).unwrap(),
            CustomError::InvalidToken
        );

        require!(
            ctx.accounts.user_account.balance >= amount,
            CustomError::InsufficientBalance
        );

        let bump = ctx.accounts.vault_authority.bump;
        let vault_seeds = &[b"vault_authority".as_ref(), &[bump]];
        let signer = &[&vault_seeds[..]];

        token_interface::transfer_checked(
            ctx.accounts.transfer_ctx().with_signer(signer),
            amount,
            ctx.accounts.token_mint.decimals,
        )?;

        ctx.accounts.user_account.balance -= amount;
        Ok(())
    }

    pub fn get_balance(ctx: Context<GetBalance>) -> Result<u64> {
        Ok(ctx.accounts.user_account.balance)
    }
}

#[account]
pub struct VaultAuthority {
    pub bump: u8,
}

#[account]
pub struct UserAccount {
    pub owner: Pubkey,
    pub balance: u64,
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + 32 + 8,
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,

    #[account(
        init_if_needed,
        payer = user,
        space = 8 + 1,
        seeds = [b"vault_authority"],
        bump
    )]
    pub vault_authority: Account<'info, VaultAuthority>,

    #[account(
        mut,
        token::mint = token_mint,
        token::authority = vault_authority,
        token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mint::token_program = token_program)]
    pub token_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        token::mint = token_mint,
        token::token_program = token_program,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

impl<'info> Deposit<'info> {
    pub fn transfer_ctx(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            TransferChecked {
                from: self.user_token_account.to_account_info(),
                to: self.vault.to_account_info(),
                authority: self.user.to_account_info(),
                mint: self.token_mint.to_account_info(),
            },
        )
    }
}

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,

    #[account(
        mut,
        seeds = [b"vault_authority"],
        bump = vault_authority.bump,
    )]
    pub vault_authority: Account<'info, VaultAuthority>,

    #[account(
        mut,
        token::mint = token_mint,
        token::authority = vault_authority,
        token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mint::token_program = token_program)]
    pub token_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        token::mint = token_mint,
        token::token_program = token_program,
    )]
    pub user_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

impl<'info> Withdraw<'info> {
    pub fn transfer_ctx(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            TransferChecked {
                from: self.vault.to_account_info(),
                to: self.user_token_account.to_account_info(),
                authority: self.vault_authority.to_account_info(),
                mint: self.token_mint.to_account_info(),
            },
        )
    }
}

#[derive(Accounts)]
pub struct GetBalance<'info> {
    #[account(
        seeds = [b"user", user.key().as_ref()],
        bump
    )]
    pub user_account: Account<'info, UserAccount>,
    pub user: Signer<'info>,
}

#[error_code]
pub enum CustomError {
    #[msg("Only RNDR token is allowed")]
    InvalidToken,
    #[msg("Insufficient balance")]
    InsufficientBalance,
}