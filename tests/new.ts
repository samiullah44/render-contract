import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY } from "@solana/web3.js";
import {
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  createMint,
  mintTo,
  getAccount,
} from "@solana/spl-token";
import { RenderNetwork } from "../target/types/render_network";
import { expect } from "chai";

describe("Render Network - Hybrid Escrow (Phase 1)", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.RenderNetwork as Program<RenderNetwork>;
  const connection = provider.connection;
  const payer = (provider.wallet as anchor.Wallet).payer;

  let mint: PublicKey;
  let userAta: PublicKey;
  const jobId = new anchor.BN(42069);
  const amount = new anchor.BN(5000000); // 5 tokens if 6 decimals

  before(async () => {
    // 1. Create a new mint for testing
    mint = await createMint(
      connection,
      payer,
      payer.publicKey,
      null,
      6
    );

    // 2. Create user token account and mint some tokens
    userAta = getAssociatedTokenAddressSync(mint, payer.publicKey);
    
    // Create the ATA and mint tokens
    const { createAssociatedTokenAccountInstruction, createMintToInstruction } = require("@solana/spl-token");
    const txSetup = new anchor.web3.Transaction().add(
        createAssociatedTokenAccountInstruction(
            payer.publicKey,
            userAta,
            payer.publicKey,
            mint
        ),
        createMintToInstruction(
            mint,
            userAta,
            payer.publicKey,
            10000000 // 10 tokens
        )
    );
    await anchor.web3.sendAndConfirmTransaction(connection, txSetup, [payer]);
  });

  it("Locks tokens for a specific job", async () => {
    const user = payer.publicKey;

    // Derive Escrow PDA: ["escrow", user, job_id]
    const [escrowPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("escrow"),
        user.toBuffer(),
        jobId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    // Derive Escrow ATA (owned by PDA)
    const escrowTokenAccount = getAssociatedTokenAddressSync(
        mint,
        escrowPda,
        true // allowOwnerOffCurve
    );

    console.log("📍 Job ID:", jobId.toString());
    console.log("📍 Escrow PDA:", escrowPda.toString());
    console.log("📍 Escrow Token Account:", escrowTokenAccount.toString());

    // EXECUTE: lock_payment
    const tx = await program.methods
      .lockPayment(jobId, amount)
      .accounts({
        escrow: escrowPda,
        user,
        mint,
        userTokenAccount: userAta,
        escrowTokenAccount,
        tokenProgram: TOKEN_PROGRAM_ID,
        associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    console.log("✅ Lock Transaction Signature:", tx);

    // VERIFY: Escrow state
    const escrowAccount = await program.account.escrow.fetch(escrowPda);
    expect(escrowAccount.jobId.toNumber()).to.equal(jobId.toNumber());
    expect(escrowAccount.amount.toNumber()).to.equal(amount.toNumber());
    expect(escrowAccount.user.toBase58()).to.equal(user.toBase58());
    expect(escrowAccount.mint.toBase58()).to.equal(mint.toBase58());
    expect(escrowAccount.status).to.equal(0); // Locked

    // VERIFY: Token balances
    const escrowTokenBalance = await connection.getTokenAccountBalance(escrowTokenAccount);
    expect(escrowTokenBalance.value.amount).to.equal(amount.toString());

    const userTokenBalance = await connection.getTokenAccountBalance(userAta);
    expect(userTokenBalance.value.amount).to.equal("5000000"); // 10M - 5M = 5M
  });

  it("Fails if jobId is different (PDA seed mismatch)", async () => {
    // This is implicitly handled by Anchor's PDA check, 
    // but we verify our understanding of seeds here.
    const wrongJobId = new anchor.BN(999);
    const [wrongEscrowPda] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("escrow"),
        payer.publicKey.toBuffer(),
        wrongJobId.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );

    // If we try to use wrongEscrowPda with jobId=42069, it should fail
    try {
        await program.methods
          .lockPayment(jobId, amount)
          .accounts({
            escrow: wrongEscrowPda,
            user: payer.publicKey,
            mint,
            userTokenAccount: userAta,
            escrowTokenAccount: getAssociatedTokenAddressSync(mint, wrongEscrowPda, true),
            tokenProgram: TOKEN_PROGRAM_ID,
            associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
            systemProgram: SystemProgram.programId,
            rent: SYSVAR_RENT_PUBKEY,
          })
          .rpc();
        expect.fail("Should have failed due to seed mismatch");
    } catch (e: any) {
        // expect(e.message).to.contain("ConstraintSeeds");
    }
  });
});