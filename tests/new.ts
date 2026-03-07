import * as anchor from "@coral-xyz/anchor";
import { Program, AnchorProvider, Wallet } from "@coral-xyz/anchor";
import { RenderNetwork } from "../target/types/render_network";
import {
  TOKEN_PROGRAM_ID,  // ✅ Use legacy token program (NOT TOKEN_2022_PROGRAM_ID)
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountIdempotentInstruction,
} from "@solana/spl-token";
import { PublicKey, Transaction, SystemProgram } from "@solana/web3.js";
import { assert } from "chai";

// 🎯 Configuration
const RNDR_MINT = new PublicKey("2PdTeB2ac5hf2V17Guq1Kb7YAU7mGeZPSnYgYAxvNPs1");
const DEPOSIT_AMOUNT = 500_000; // 500,000 tokens
const TOKEN_PROGRAM = TOKEN_PROGRAM_ID; // ✅ Legacy SPL Token program

describe("render_network - Deposit RNDR", () => {
  const provider = AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.RenderNetwork as Program<RenderNetwork>;
  const wallet = provider.wallet as Wallet;
  const user = wallet.publicKey;

  // 📍 PDAs
  const [userAccountPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("user"), user.toBuffer()],
    program.programId
  );

  const [vaultAuthorityPda] = PublicKey.findProgramAddressSync(
    [Buffer.from("vault_authority")],
    program.programId
  );

  // 🏦 Token Accounts (using LEGACY token program)
  const userTokenAccount = getAssociatedTokenAddressSync(
    RNDR_MINT,
    user,
    false,
    TOKEN_PROGRAM // ✅ Pass correct token program
  );

  const vaultTokenAccount = getAssociatedTokenAddressSync(
    RNDR_MINT,
    vaultAuthorityPda,
    true,
    TOKEN_PROGRAM // ✅ Pass correct token program
  );

  // 🔍 Helper: Safely get token balance
  async function getTokenBalanceSafely(tokenAccount: PublicKey): Promise<number> {
    try {
      const balance = await provider.connection.getTokenAccountBalance(tokenAccount);
      return balance.value.uiAmount || 0;
    } catch (e: any) {
      if (e.message?.includes("could not find account")) {
        return 0;
      }
      throw e;
    }
  }

  it("Deposits 500,000 RNDR tokens", async () => {
    console.log("\n📍 User:", user.toString());
    console.log("📍 User Account PDA:", userAccountPda.toString());
    console.log("📍 Vault Authority PDA:", vaultAuthorityPda.toString());
    console.log("📍 User Token Account:", userTokenAccount.toString());
    console.log("📍 Vault Token Account:", vaultTokenAccount.toString());
    console.log("💰 Deposit Amount:", DEPOSIT_AMOUNT.toLocaleString(), "RNDR\n");

    // 🔍 Check initial token balance (safe)
    const initialUserTokenBalance = await getTokenBalanceSafely(userTokenAccount);
    console.log("📊 Initial User Token Balance:", initialUserTokenBalance.toLocaleString());

    // 🔍 Check initial program account balance
    let initialProgramBalance = BigInt(0);
    try {
      const userAccount = await program.account.userAccount.fetch(userAccountPda);
      initialProgramBalance = userAccount.balance;
      console.log("📊 Initial Program Account Balance:", initialProgramBalance.toString());
    } catch (e) {
      console.log("ℹ️  User account will be created via init_if_needed");
    }

    // 🛠️ Build the deposit instruction
    const depositIx = await program.methods
      .depositRndr(new anchor.BN(DEPOSIT_AMOUNT))
      .accounts({
        user: user,
        userAccount: userAccountPda,
        vaultAuthority: vaultAuthorityPda,
        vault: vaultTokenAccount,
        tokenMint: RNDR_MINT,
        userTokenAccount: userTokenAccount,
        tokenProgram: TOKEN_PROGRAM, // ✅ Use legacy token program
        systemProgram: SystemProgram.programId,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .instruction();

    // 📝 Add idempotent ATA creation instructions
    const createUserAtaIx = createAssociatedTokenAccountIdempotentInstruction(
      user,
      userTokenAccount,
      user,
      RNDR_MINT,
      TOKEN_PROGRAM // ✅ Use legacy token program
    );

    const createVaultAtaIx = createAssociatedTokenAccountIdempotentInstruction(
      user,
      vaultTokenAccount,
      vaultAuthorityPda,
      RNDR_MINT,
      TOKEN_PROGRAM // ✅ Use legacy token program
    );

    // 🔗 Combine all instructions
    const tx = new Transaction().add(createUserAtaIx).add(createVaultAtaIx).add(depositIx);

    // 🚀 Send and confirm
    const txSignature = await provider.sendAndConfirm(tx);
    console.log("\n✅ Transaction Signature:", txSignature);
    console.log("🔗 Explorer: https://explorer.solana.com/tx/" + txSignature + "?cluster=devnet\n");

    // ✅ Verify final balances
    const finalUserTokenBalance = await getTokenBalanceSafely(userTokenAccount);
    console.log("📊 Final User Token Balance:", finalUserTokenBalance.toLocaleString());

    const userAccount = await program.account.userAccount.fetch(userAccountPda);
    console.log("📊 Final Program Account Balance:", userAccount.balance.toString());

    // 🧪 Assertions
    assert.equal(
      userAccount.balance.toNumber(),
      Number(initialProgramBalance) + DEPOSIT_AMOUNT,
      "Program balance should increase by deposit amount"
    );

    console.log("\n🎉 Deposit test passed!\n");
  });
});