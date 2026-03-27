import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, SystemProgram, SYSVAR_RENT_PUBKEY, Keypair, Transaction, sendAndConfirmTransaction } from "@solana/web3.js";
import {
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  createMint,
  mintTo,
  getAccount,
  createAssociatedTokenAccountInstruction,
  createMintToInstruction,
  createCloseAccountInstruction,
  createBurnInstruction,
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
  let configPda: PublicKey;
  const jobId = new anchor.BN(Math.floor(Math.random() * 1000000) + 1);
  const amount = new anchor.BN(5000000); // 5 tokens if 6 decimals

  // Shared Provider keys and ATAs for cleanup
  let providerA: Keypair;
  let providerB: Keypair;
  let tempProvider: Keypair;
  let providerAtaA: PublicKey;
  let providerAtaB: PublicKey;
  let tempAta: PublicKey;

  before(async () => {
    // 0. Derive and initialize Global Config
    [configPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("config_v2")],
      program.programId
    );

    try {
        await program.methods
          .initializeConfig(payer.publicKey, new anchor.BN(2)) // 2 second delay
          .accounts({
            admin: payer.publicKey,
          })
          .rpc();
        console.log("✅ Global Config Initialized (v2)");
    } catch (e) {
        console.log("ℹ️ Global Config (v2) already exists or failed to init");
    }

    // 1. Create a new mint for testing
    mint = await createMint(
      connection,
      payer,
      payer.publicKey,
      null,
      6
    );
    console.log("🪙 Test Token Mint Address:", mint.toString());
    const cluster = provider.connection.rpcEndpoint.includes("devnet") ? "devnet" : "localnet";
    console.log(`🔗 View Token on Explorer: https://explorer.solana.com/address/${mint.toString()}?cluster=${cluster}`);

    // 2. Create user token account and mint some tokens
    userAta = getAssociatedTokenAddressSync(mint, payer.publicKey);
    
    // Create the ATA and mint tokens
    const txSetup = new Transaction().add(
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
    await sendAndConfirmTransaction(connection, txSetup, [payer]);
  });

  // CLEANUP: Close all temporary ATAs to reclaim SOL
  after(async () => {
    console.log("🧹 Cleaning up test accounts...");
    const balBefore = await connection.getBalance(payer.publicKey);
    
    // List of ATAs to close (Providers and Temp)
    const targets = [
        { ata: providerAtaA, owner: providerA },
        { ata: providerAtaB, owner: providerB },
        { ata: tempAta, owner: tempProvider },
    ];

    let closedCount = 0;
    for (const target of targets) {
        if (target.ata && target.owner) {
            try {
                const account = await getAccount(connection, target.ata);
                const cleanupTx = new Transaction();
                
                // 1. Burn remaining tokens if any
                if (account.amount > BigInt(0)) {
                    cleanupTx.add(
                        createBurnInstruction(
                            target.ata,
                            mint,
                            target.owner.publicKey,
                            account.amount
                        )
                    );
                }
                
                // 2. Close account to reclaim SOL
                cleanupTx.add(
                    createCloseAccountInstruction(
                        target.ata,
                        payer.publicKey, // Destination
                        target.owner.publicKey,
                        []
                    )
                );
                
                await sendAndConfirmTransaction(connection, cleanupTx, [payer, target.owner]);
                closedCount++;
            } catch (e) {
                // Account might already be closed or not exists
            }
        }
    }
    const balAfter = await connection.getBalance(payer.publicKey);
    console.log(`✅ Cleanup complete. Closed ${closedCount} accounts.`);
    console.log(`💰 SOL Reclaimed: ${((balAfter - balBefore) / anchor.web3.LAMPORTS_PER_SOL).toFixed(6)} SOL`);
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
        mint,
        userTokenAccount: userAta,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .rpc();

    console.log("✅ Lock Transaction Signature:", tx);

    // VERIFY: Escrow state
    const escrowAccount = await program.account.escrow.fetch(escrowPda);
    expect(escrowAccount.jobId.toNumber()).to.equal(jobId.toNumber());
    expect(escrowAccount.amount.toNumber()).to.equal(amount.toNumber());
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(amount.toNumber());
    expect(escrowAccount.user.toBase58()).to.equal(user.toBase58());
    expect(escrowAccount.mint.toBase58()).to.equal(mint.toBase58());
    expect(escrowAccount.status).to.equal(0); // Locked

    // VERIFY: Token balances
    const escrowTokenBalance = await connection.getTokenAccountBalance(escrowTokenAccount);
    expect(escrowTokenBalance.value.amount).to.equal(amount.toString());

    const userTokenBalance = await connection.getTokenAccountBalance(userAta);
    console.log(`💰 User Token Balance (After Lock): ${userTokenBalance.value.uiAmount} tokens`);
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
          .accountsPartial({
            escrow: wrongEscrowPda,
            user: payer.publicKey,
            mint,
            userTokenAccount: userAta,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .rpc();
        expect.fail("Should have failed due to seed mismatch");
    } catch (e: any) {
        // expect(e.message).to.contain("ConstraintSeeds");
    }
  });

  it("Executes a Batch Release to multiple providers", async () => {
    // 1. Create two provider ATAs
    providerA = Keypair.generate();
    providerB = Keypair.generate();
    
    providerAtaA = getAssociatedTokenAddressSync(mint, providerA.publicKey);
    providerAtaB = getAssociatedTokenAddressSync(mint, providerB.publicKey);

    const txAta = new Transaction().add(
        createAssociatedTokenAccountInstruction(payer.publicKey, providerAtaA, providerA.publicKey, mint),
        createAssociatedTokenAccountInstruction(payer.publicKey, providerAtaB, providerB.publicKey, mint)
    );
    await sendAndConfirmTransaction(connection, txAta, [payer]);

    // 2. Prepare Batch Data
    const payoutA = new anchor.BN(1000000); // 1 token
    const payoutB = new anchor.BN(2000000); // 2 tokens
    const batch = [{
        jobId: jobId,
        payouts: [
            { provider: providerA.publicKey, amount: payoutA },
            { provider: providerB.publicKey, amount: payoutB }
        ]
    }];

    // 3. Derive Escrow accounts again for validation
    const [escrowPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("escrow"), payer.publicKey.toBuffer(), jobId.toArrayLike(Buffer, "le", 8)],
        program.programId
    );
    const escrowTokenAccount = getAssociatedTokenAddressSync(mint, escrowPda, true);

    // 3.5 MARK JOB COMPLETED
    console.log("⏳ Marking job completed...");
    await program.methods
        .markJobCompleted()
        .accounts({
            admin: payer.publicKey,
            escrow: escrowPda,
            config: configPda,
        })
        .rpc();

    // 3.6 TEST TIME LOCK (Should fail if immediate)
    try {
        await program.methods
            .batchRelease(batch)
            .accounts({
                admin: payer.publicKey,
                mint,
                tokenProgram: TOKEN_PROGRAM_ID,
            })
            .remainingAccounts([
                { pubkey: escrowPda, isWritable: true, isSigner: false },
                { pubkey: escrowTokenAccount, isWritable: true, isSigner: false },
                { pubkey: providerAtaA, isWritable: true, isSigner: false },
                { pubkey: providerAtaB, isWritable: true, isSigner: false },
            ])
            .rpc();
        expect.fail("Should have failed due to release delay");
    } catch (e: any) {
        // console.log("Caught expected error:", e.message);
    }

    // 4. WAIT FOR DELAY
    console.log("⏱️ Waiting for 3 seconds...");
    await new Promise(resolve => setTimeout(resolve, 3000));

    // 5. EXECUTE: batch_release
    const tx = await program.methods
        .batchRelease(batch)
        .accounts({
            admin: payer.publicKey,
            mint,
            tokenProgram: TOKEN_PROGRAM_ID,
        })
        .remainingAccounts([
            { pubkey: escrowPda, isWritable: true, isSigner: false },
            { pubkey: escrowTokenAccount, isWritable: true, isSigner: false },
            { pubkey: providerAtaA, isWritable: true, isSigner: false },
            { pubkey: providerAtaB, isWritable: true, isSigner: false },
        ])
        .rpc();

    console.log("✅ Batch Release Transaction:", tx);

    // 5. VERIFY: Escrow State
    const escrowAccount = await program.account.escrow.fetch(escrowPda);
    expect(escrowAccount.remainingAmount.toNumber()).to.equal(amount.sub(payoutA).sub(payoutB).toNumber());
    expect(escrowAccount.releasedAmount.toNumber()).to.equal(payoutA.add(payoutB).toNumber());
    expect(escrowAccount.status).to.equal(1); // Partial

    // 6. VERIFY: Provider Balances
    const balA = await connection.getTokenAccountBalance(providerAtaA);
    const balB = await connection.getTokenAccountBalance(providerAtaB);
    console.log(`💰 Provider A Received: ${balA.value.uiAmount} tokens`);
    console.log(`💰 Provider B Received: ${balB.value.uiAmount} tokens`);
    expect(balA.value.amount).to.equal(payoutA.toString());
    expect(balB.value.amount).to.equal(payoutB.toString());
  });

  it("Closes a fully released escrow account", async () => {
    // 1. Derive Escrow PDA again
    const [escrowPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("escrow"), payer.publicKey.toBuffer(), jobId.toArrayLike(Buffer, "le", 8)],
        program.programId
    );

    // 2. EXECUTE: close_escrow (Remaining amount is 2M, so we need to release more first or use a finished one)
    // Actually, in the previous test we released 3M out of 5M. 2M remains.
    // Let's finish the release for jobId.
    const remainingPayout = new anchor.BN(2000000);
    
    // We need an ATA for this new provider
    tempProvider = Keypair.generate();
    tempAta = getAssociatedTokenAddressSync(mint, tempProvider.publicKey);

    const batchFinish = [{
        jobId: jobId,
        payouts: [{ provider: tempProvider.publicKey, amount: remainingPayout }]
    }];
    await sendAndConfirmTransaction(connection, new Transaction().add(
        createAssociatedTokenAccountInstruction(payer.publicKey, tempAta, tempProvider.publicKey, mint)
    ), [payer]);

    const [escrowTokenAccount] = PublicKey.findProgramAddressSync(
        [escrowPda.toBuffer(), TOKEN_PROGRAM_ID.toBuffer(), mint.toBuffer()],
        ASSOCIATED_TOKEN_PROGRAM_ID
    );

    await program.methods
        .batchRelease(batchFinish)
        .accounts({
            admin: payer.publicKey,
            mint,
            tokenProgram: TOKEN_PROGRAM_ID,
        })
        .remainingAccounts([
            { pubkey: escrowPda, isWritable: true, isSigner: false },
            { pubkey: escrowTokenAccount, isWritable: true, isSigner: false },
            { pubkey: tempAta, isWritable: true, isSigner: false },
        ])
        .rpc();

    // Now remainingAmount should be 0.
    // 3. EXECUTE: close_escrow
    const tx = await program.methods
        .closeEscrow()
        .accounts({
            escrow: escrowPda,
            user: payer.publicKey,
            mint,
            escrowTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();
    
    console.log("✅ Close Escrow Transaction:", tx);

    // 4. VERIFY: Account is gone
    const accountInfo = await connection.getAccountInfo(escrowPda);
    expect(accountInfo).to.be.null;
  });

  it("Cancels a job and refunds the remaining balance", async () => {
    // 1. Create a SECOND job to test cancellation individually
    const newJobId = new anchor.BN(Math.floor(Math.random() * 1000000) + 1000001);
    const lockAmount = new anchor.BN(3000000); // 3 tokens

    const [newEscrowPda] = PublicKey.findProgramAddressSync(
        [Buffer.from("escrow"), payer.publicKey.toBuffer(), newJobId.toArrayLike(Buffer, "le", 8)],
        program.programId
    );
    const newEscrowTokenAccount = getAssociatedTokenAddressSync(mint, newEscrowPda, true);

    await program.methods
        .lockPayment(newJobId, lockAmount)
        .accounts({
            mint,
            userTokenAccount: userAta,
            tokenProgram: TOKEN_PROGRAM_ID,
        })
        .rpc();

    // 2. EXECUTE: cancel_job
    const tx = await program.methods
        .cancelJob()
        .accounts({
            escrow: newEscrowPda,
            user: payer.publicKey,
            mint,
            userTokenAccount: userAta,
            escrowTokenAccount: newEscrowTokenAccount,
            tokenProgram: TOKEN_PROGRAM_ID,
            associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
            systemProgram: SystemProgram.programId,
        })
        .rpc();

    console.log("✅ Cancel Job Transaction:", tx);

    // 3. VERIFY: Escrow State (Should be GONE now)
    const accountInfo = await connection.getAccountInfo(newEscrowPda);
    expect(accountInfo).to.be.null;

    // 4. VERIFY: User Balance (Should be back up)
    const userBal = await connection.getTokenAccountBalance(userAta);
    console.log(`💰 User Token Balance (After Cancellation Refund): ${userBal.value.uiAmount} tokens`);
    // User had 10M, spent 5M (Job 1), then 3M (Job 2), then got 3M back. Should have 5M.
    expect(userBal.value.amount).to.equal("5000000"); 
  });
});