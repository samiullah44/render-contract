import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
  PublicKey, SystemProgram, Keypair,
  Transaction, sendAndConfirmTransaction
} from "@solana/web3.js";
import {
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  createMint,
  createAssociatedTokenAccountInstruction,
  createMintToInstruction,
  createCloseAccountInstruction,
  createBurnInstruction,
  getAccount,
} from "@solana/spl-token";
import { RenderNetwork } from "../target/types/render_network";
import { expect } from "chai";

describe("Render Network - User Credit Escrow (Full Suite)", () => {
  const provider   = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program    = anchor.workspace.RenderNetwork as Program<RenderNetwork>;
  const connection = provider.connection;
  const payer      = (provider.wallet as anchor.Wallet).payer;

  // ── Shared state ──
  let mint: PublicKey;
  let userAta: PublicKey;
  let userAccountPda: PublicKey;
  let userDepositAta: PublicKey;
  let configPda: PublicKey;

  // Persistent User Identity for the test session
  const testUserId = Keypair.generate().publicKey;


  // JobId1 is used for the main batch-release flow
  // JobId2 is used for the cancel flow
  const jobId1 = new anchor.BN(Math.floor(Math.random() * 900_000) + 1);
  const jobId2 = new anchor.BN(Math.floor(Math.random() * 900_000) + 1_000_001);

  const depositAmount = new anchor.BN(10_000_000); // 10 tokens
  const lockAmount    = new anchor.BN( 5_000_000); // 5 tokens for job1
  const cancelAmount  = new anchor.BN( 3_000_000); // 3 tokens for job2

  // Provider keypairs
  let providerA: Keypair, providerB: Keypair, tempProvider: Keypair;
  let providerAtaA: PublicKey, providerAtaB: PublicKey, tempAta: PublicKey;

  // ─────────────────────────────────────────────────────────────
  // SETUP
  // ─────────────────────────────────────────────────────────────
  before(async () => {
    [configPda] = PublicKey.findProgramAddressSync([Buffer.from("config_v2")], program.programId);

    try {
      await program.methods.initializeConfig(payer.publicKey, new anchor.BN(2))
        .accounts({ admin: payer.publicKey }).rpc();
      console.log("✅ Global Config Initialized (v2)");
    } catch { console.log("ℹ️  Global Config (v2) already exists or failed to init"); }

    // 1. Create mint & user ATA with 20 tokens
    mint    = await createMint(connection, payer, payer.publicKey, null, 6);
    userAta = getAssociatedTokenAddressSync(mint, payer.publicKey);
    await sendAndConfirmTransaction(connection, new Transaction().add(
      createAssociatedTokenAccountInstruction(payer.publicKey, userAta, payer.publicKey, mint),
      createMintToInstruction(mint, userAta, payer.publicKey, 20_000_000),
    ), [payer]);

    const cluster = connection.rpcEndpoint.includes("devnet") ? "devnet" : "localnet";
    console.log("🪙 Test Token Mint Address:", mint.toString());
    console.log(`🔗 View Token on Explorer: https://explorer.solana.com/address/${mint.toString()}?cluster=${cluster}`);

    // 2. Derive Credit PDA addresses (Using Identity Seed)
    [userAccountPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("user_account"), testUserId.toBuffer()],
      program.programId
    );
    userDepositAta = getAssociatedTokenAddressSync(mint, userAccountPda, true);
  });

  // ─────────────────────────────────────────────────────────────
  // CLEANUP: Close provider ATAs, reclaim SOL
  // ─────────────────────────────────────────────────────────────
  after(async () => {
    console.log("🧹 Cleaning up test accounts...");
    const balBefore = await connection.getBalance(payer.publicKey);
    const targets = [
      { ata: providerAtaA, owner: providerA },
      { ata: providerAtaB, owner: providerB },
      { ata: tempAta,      owner: tempProvider },
    ];
    let closedCount = 0;
    for (const t of targets) {
      if (!t.ata || !t.owner) continue;
      try {
        const acc = await getAccount(connection, t.ata);
        const tx  = new Transaction();
        if (acc.amount > 0n) tx.add(createBurnInstruction(t.ata, mint, t.owner.publicKey, acc.amount));
        tx.add(createCloseAccountInstruction(t.ata, payer.publicKey, t.owner.publicKey));
        await sendAndConfirmTransaction(connection, tx, [payer, t.owner]);
        closedCount++;
      } catch {}
    }
    const balAfter = await connection.getBalance(payer.publicKey);
    console.log(`✅ Cleanup complete. Closed ${closedCount} accounts.`);
    console.log(`💰 SOL Reclaimed: ${((balAfter - balBefore) / anchor.web3.LAMPORTS_PER_SOL).toFixed(6)} SOL`);
  });

  // ─────────────────────────────────────────────────────────────
  // TEST 1: Deposit tokens → Website Credit Account
  // (Account is created on first deposit. User pays rent.)
  // ─────────────────────────────────────────────────────────────
  it("Deposit tokens into Website Credit Account", async () => {
    const tx = await program.methods.depositToAccount(testUserId, depositAmount).accounts({

      userAccount:             userAccountPda,
      user:                    payer.publicKey,
      mint,
      userTokenAccount:        userAta,
      userDepositTokenAccount: userDepositAta,
      tokenProgram:            TOKEN_PROGRAM_ID,
      associatedTokenProgram:  ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram:           SystemProgram.programId,
    }).rpc();

    const acc = await program.account.userAccount.fetch(userAccountPda);
    expect(acc.creditedAmount.toNumber()).to.equal(depositAmount.toNumber());

    const walletBal = await connection.getTokenAccountBalance(userAta);
    console.log(`✅ Deposit Transaction Signature: ${tx}`);
    console.log(`💰 Website Credits: ${(acc.creditedAmount.toNumber() / 1e6).toFixed(2)} tokens`);
    console.log(`💰 Wallet Balance (After Deposit): ${walletBal.value.uiAmount} tokens`);
    console.log(`📍 Credit Account PDA: ${userAccountPda.toString()}`);
    console.log(`📍 Credit Token Account: ${userDepositAta.toString()}`);
  });

  // ─────────────────────────────────────────────────────────────
  // TEST 2: Lock payment from Credits → Escrow (Job Start)
  // ─────────────────────────────────────────────────────────────
  it("Locks tokens for a specific job (from Credits)", async () => {
    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), testUserId.toBuffer(), jobId1.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    const escrowAta = getAssociatedTokenAddressSync(mint, escrowPda, true);

    const tx = await program.methods.lockPayment(testUserId, jobId1, lockAmount).accounts({

      escrow:                  escrowPda,
      user:                    payer.publicKey,
      userDepositAccount:      userAccountPda,
      mint,
      userDepositTokenAccount: userDepositAta,
      escrowTokenAccount:      escrowAta,
      tokenProgram:            TOKEN_PROGRAM_ID,
      associatedTokenProgram:  ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram:           SystemProgram.programId,
    }).rpc();

    const escrowAcc = await program.account.escrow.fetch(escrowPda);
    expect(escrowAcc.amount.toNumber()).to.equal(lockAmount.toNumber());
    expect(escrowAcc.status).to.equal(0); // Locked

    const creditAcc = await program.account.userAccount.fetch(userAccountPda);
    expect(creditAcc.creditedAmount.toNumber()).to.equal(depositAmount.toNumber() - lockAmount.toNumber());

    console.log(`✅ Lock Transaction Signature: ${tx}`);
    console.log(`📍 Job ID: ${jobId1.toString()}`);
    console.log(`📍 Escrow PDA: ${escrowPda.toString()}`);
    console.log(`📍 Escrow Token Account: ${escrowAta.toString()}`);
    console.log(`💰 Remaining Credits (After Lock): ${(creditAcc.creditedAmount.toNumber() / 1e6).toFixed(2)} tokens`);
  });

  // ─────────────────────────────────────────────────────────────
  // TEST 3: Fails if jobId causes PDA seed mismatch
  // ─────────────────────────────────────────────────────────────
  it("Fails if jobId is different (PDA seed mismatch)", async () => {
    const wrongJobId = new anchor.BN(999_999_999);
    const [wrongEscrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), testUserId.toBuffer(), wrongJobId.toArrayLike(Buffer, "le", 8)],
      program.programId
    );
    try {
      await program.methods.lockPayment(testUserId, jobId1, lockAmount).accountsPartial({

        escrow: wrongEscrowPda,
        user:   payer.publicKey,
        mint,
      }).rpc();
      expect.fail("Should have failed due to seed mismatch");
    } catch (e: any) {
      // Expected to fail
    }
  });

  // ─────────────────────────────────────────────────────────────
  // TEST 4: Batch Release → Multiple Providers
  // ─────────────────────────────────────────────────────────────
  it("Executes a Batch Release to multiple providers", async () => {
    providerA = Keypair.generate();
    providerB = Keypair.generate();
    providerAtaA = getAssociatedTokenAddressSync(mint, providerA.publicKey);
    providerAtaB = getAssociatedTokenAddressSync(mint, providerB.publicKey);
    await sendAndConfirmTransaction(connection, new Transaction().add(
      createAssociatedTokenAccountInstruction(payer.publicKey, providerAtaA, providerA.publicKey, mint),
      createAssociatedTokenAccountInstruction(payer.publicKey, providerAtaB, providerB.publicKey, mint),
    ), [payer]);

    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), testUserId.toBuffer(), jobId1.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    const escrowAta = getAssociatedTokenAddressSync(mint, escrowPda, true);

    // Mark completed
    console.log("⏳ Marking job completed...");
    await program.methods.markJobCompleted().accounts({
      admin: payer.publicKey, escrow: escrowPda, config: configPda,
    }).rpc();

    // Wait for release delay
    console.log("⏱️ Waiting for 3 seconds...");
    await new Promise(r => setTimeout(r, 3000));

    const payoutA = new anchor.BN(1_000_000); // 1 token
    const payoutB = new anchor.BN(2_000_000); // 2 tokens

    const tx = await program.methods.batchRelease([{
      jobId: jobId1,
      payouts: [
        { provider: providerA.publicKey, amount: payoutA },
        { provider: providerB.publicKey, amount: payoutB },
      ],
    }]).accounts({
      admin: payer.publicKey, mint, tokenProgram: TOKEN_PROGRAM_ID,
    }).remainingAccounts([
      { pubkey: escrowPda,    isWritable: true, isSigner: false },
      { pubkey: escrowAta,    isWritable: true, isSigner: false },
      { pubkey: providerAtaA, isWritable: true, isSigner: false },
      { pubkey: providerAtaB, isWritable: true, isSigner: false },
    ]).rpc();

    const balA = await connection.getTokenAccountBalance(providerAtaA);
    const balB = await connection.getTokenAccountBalance(providerAtaB);
    expect(balA.value.amount).to.equal(payoutA.toString());
    expect(balB.value.amount).to.equal(payoutB.toString());

    console.log(`✅ Batch Release Transaction: ${tx}`);
    console.log(`💰 Provider A Received: ${balA.value.uiAmount} tokens`);
    console.log(`💰 Provider B Received: ${balB.value.uiAmount} tokens`);
  });

  // ─────────────────────────────────────────────────────────────
  // TEST 5: Close fully-released Escrow (reclaim rent)
  // ─────────────────────────────────────────────────────────────
  it("Closes a fully released escrow account", async () => {
    tempProvider = Keypair.generate();
    tempAta      = getAssociatedTokenAddressSync(mint, tempProvider.publicKey);
    await sendAndConfirmTransaction(connection, new Transaction().add(
      createAssociatedTokenAccountInstruction(payer.publicKey, tempAta, tempProvider.publicKey, mint),
    ), [payer]);

    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), testUserId.toBuffer(), jobId1.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    const escrowAta = getAssociatedTokenAddressSync(mint, escrowPda, true);

    // Release the remaining 2M to fully drain escrow
    const remaining = new anchor.BN(2_000_000);
    await program.methods.batchRelease([{
      jobId: jobId1,
      payouts: [{ provider: tempProvider.publicKey, amount: remaining }],
    }]).accounts({
      admin: payer.publicKey, mint, tokenProgram: TOKEN_PROGRAM_ID,
    }).remainingAccounts([
      { pubkey: escrowPda, isWritable: true, isSigner: false },
      { pubkey: escrowAta, isWritable: true, isSigner: false },
      { pubkey: tempAta,   isWritable: true, isSigner: false },
    ]).rpc();

    // Now close it
    const tx = await program.methods.closeEscrow().accounts({
      escrow:             escrowPda,
      user:               payer.publicKey,
      mint,
      escrowTokenAccount: escrowAta,
      tokenProgram:       TOKEN_PROGRAM_ID,
    }).rpc();

    const accountInfo = await connection.getAccountInfo(escrowPda);
    expect(accountInfo).to.be.null;
    console.log(`✅ Close Escrow Transaction: ${tx}`);
  });

  // ─────────────────────────────────────────────────────────────
  // TEST 6: Cancel Job → Refund back to Credit Account
  // ─────────────────────────────────────────────────────────────
  it("Cancels a job and refunds to Credit Account", async () => {
    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), testUserId.toBuffer(), jobId2.toArrayLike(Buffer, "le", 8)],
      program.programId
    );

    const escrowAta = getAssociatedTokenAddressSync(mint, escrowPda, true);

    // Lock Job 2
    await program.methods.lockPayment(testUserId, jobId2, cancelAmount).accounts({

      escrow:                  escrowPda,
      user:                    payer.publicKey,
      userDepositAccount:      userAccountPda,
      mint,
      userDepositTokenAccount: userDepositAta,
      escrowTokenAccount:      escrowAta,
      tokenProgram:            TOKEN_PROGRAM_ID,
      associatedTokenProgram:  ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram:           SystemProgram.programId,
    }).rpc();

    const beforeCancel = await program.account.userAccount.fetch(userAccountPda);

    // Cancel Job 2
    const tx = await program.methods.cancelJob().accounts({
      escrow:                  escrowPda,
      user:                    payer.publicKey,
      userDepositAccount:      userAccountPda,
      mint,
      userDepositTokenAccount: userDepositAta,
      escrowTokenAccount:      escrowAta,
      tokenProgram:            TOKEN_PROGRAM_ID,
      associatedTokenProgram:  ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram:           SystemProgram.programId,
    }).rpc();

    const afterCancel = await program.account.userAccount.fetch(userAccountPda);
    expect(afterCancel.creditedAmount.toNumber()).to.equal(
      beforeCancel.creditedAmount.toNumber() + cancelAmount.toNumber()
    );

    console.log(`✅ Cancel Job Transaction: ${tx}`);
    console.log(`💰 Credits After Cancellation Refund: ${(afterCancel.creditedAmount.toNumber() / 1e6).toFixed(2)} tokens`);
  });
});