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
  getAccount,
} from "@solana/spl-token";
import { RenderNetwork } from "../target/types/render_network";
import { expect } from "chai";

describe("Render Network - Logical Escrow (Verification)", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.RenderNetwork as Program<RenderNetwork>;
  const connection = provider.connection;
  const payer = (provider.wallet as anchor.Wallet).payer;

  // ── Shared state ──
  let mint: PublicKey;
  let userAta: PublicKey;
  let userAccountPda: PublicKey;
  let userDepositAta: PublicKey;
  let configPda: PublicKey;
  let feeCollector: Keypair;
  let feeCollectorAta: PublicKey;

  const testUserId = Keypair.generate().publicKey;
  const platformFeeBps = 200; // 2%
  const depositAmount = new anchor.BN(10_000_000); // 10 tokens

  let providerA: Keypair, providerB: Keypair;
  let providerAtaA: PublicKey, providerAtaB: PublicKey;

  before(async () => {
    // 1. Initialize Global Config (2% fee)
    [configPda] = PublicKey.findProgramAddressSync([Buffer.from("config_v3")], program.programId);
    try {
      const configAcc = await program.account.globalConfig.fetch(configPda);
      console.log("ℹ️ Global Config already exists");
      // Use the fee collector already stored on-chain
      feeCollector = { publicKey: configAcc.feeCollector } as any;
    } catch (e) {
      console.log("🚀 Initializing Global Config (2% Fee)...");
      feeCollector = Keypair.generate();
      try {
          const tx = await program.methods
            .initializeGlobalConfig(new anchor.BN(200)) // 2% fee
            .accounts({
              config: configPda,
              admin: provider.wallet.publicKey,
              feeCollector: feeCollector.publicKey,
              systemProgram: anchor.web3.SystemProgram.programId,
            })
            .rpc();
          console.log(`✅ Config Initialized! TX: ${tx}`);
      } catch (initErr) {
          console.log(`ℹ️ Initialization result: ${initErr.message}`);
      }
    }

    // 2. Setup Mint & User ATA
    mint = await createMint(connection, payer, payer.publicKey, null, 6);
    userAta = getAssociatedTokenAddressSync(mint, payer.publicKey);
    feeCollectorAta = getAssociatedTokenAddressSync(mint, feeCollector.publicKey);

    await sendAndConfirmTransaction(connection, new Transaction().add(
      createAssociatedTokenAccountInstruction(payer.publicKey, userAta, payer.publicKey, mint),
      createAssociatedTokenAccountInstruction(payer.publicKey, feeCollectorAta, feeCollector.publicKey, mint),
      createMintToInstruction(mint, userAta, payer.publicKey, 20_000_000),
    ), [payer]);

    // 3. Derive User PDA
    [userAccountPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("user_account_v2"), testUserId.toBuffer()],
      program.programId
    );
    userDepositAta = getAssociatedTokenAddressSync(mint, userAccountPda, true);
  });

  it("Step 1: Deposit tokens into Logical Vault (Credit Account)", async () => {
    await program.methods.depositToAccount(testUserId, depositAmount).accounts({
      userAccount: userAccountPda,
      user: payer.publicKey,
      mint,
      userTokenAccount: userAta,
      userDepositTokenAccount: userDepositAta,
      tokenProgram: TOKEN_PROGRAM_ID,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    }).rpc();

    const acc = await program.account.userAccount.fetch(userAccountPda);
    expect(acc.creditedAmount.toNumber()).to.equal(depositAmount.toNumber());
    console.log(`✅ Deposited ${depositAmount.toNumber()} to Logical Vault`);
  });

  it("Step 2: Admin Lock Payment (Zero-Prompt Simulation)", async () => {
    const jobId = new anchor.BN(100); // Higher nonce
    const lockAmount = new anchor.BN(5_000_000); // 5 tokens

    await program.methods.adminLockPayment(jobId, lockAmount).accounts({
      config: configPda,
      admin: payer.publicKey,
      userAccount: userAccountPda,
    }).rpc();

    const acc = await program.account.userAccount.fetch(userAccountPda);
    expect(acc.lockedAmount.toNumber()).to.equal(lockAmount.toNumber());
    expect(acc.lastJobNonce.toNumber()).to.equal(jobId.toNumber());
    console.log(`✅ Admin locked ${lockAmount.toNumber()} (Nonce: ${jobId.toNumber()})`);
  });

  it("Step 3: Replay Protection Check", async () => {
    const oldJobId = new anchor.BN(50); // Lower than current (100)
    const lockAmount = new anchor.BN(1_000_000);

    try {
      await program.methods.adminLockPayment(oldJobId, lockAmount).accounts({
        config: configPda,
        admin: payer.publicKey,
        userAccount: userAccountPda,
      }).rpc();
      expect.fail("Should have failed due to Replay Protection");
    } catch (e: any) {
      expect(e.message).to.contain("ReplayProtection");
      console.log("✅ Replay Protection working (Nonce rejected)");
    }
  });

  it("Step 3b: Admin Cancel/Unlock Payment", async () => {
    const unlockAmount = new anchor.BN(2_000_000); // 2 tokens
    const accBefore = await program.account.userAccount.fetch(userAccountPda);
    const initialLocked = accBefore.lockedAmount;

    await program.methods.adminCancelPayment(new anchor.BN(100), unlockAmount).accounts({
      config: configPda,
      admin: payer.publicKey,
      userAccount: userAccountPda,
    }).rpc();

    const accAfter = await program.account.userAccount.fetch(userAccountPda);
    expect(accAfter.lockedAmount.toNumber()).to.equal(initialLocked.toNumber() - unlockAmount.toNumber());
    console.log(`✅ Admin successfully unlocked ${unlockAmount.toNumber()} (Current Locked: ${accAfter.lockedAmount.toNumber()})`);
  });

  it("Step 4: Batch Payout with 2% Platform Fee", async () => {
    providerA = Keypair.generate();
    providerB = Keypair.generate();
    providerAtaA = getAssociatedTokenAddressSync(mint, providerA.publicKey);
    providerAtaB = getAssociatedTokenAddressSync(mint, providerB.publicKey);

    await sendAndConfirmTransaction(connection, new Transaction().add(
      createAssociatedTokenAccountInstruction(payer.publicKey, providerAtaA, providerA.publicKey, mint),
      createAssociatedTokenAccountInstruction(payer.publicKey, providerAtaB, providerB.publicKey, mint),
    ), [payer]);

    const jobId = new anchor.BN(100);
    const payoutA = new anchor.BN(2_000_000); // 2 tokens
    const payoutB = new anchor.BN(1_000_000); // 1 token
    const totalPayout = payoutA.add(payoutB); // 3 tokens total

    // Execute Batch Release
    await program.methods.batchRelease([{
      jobId,
      payouts: [
        { provider: providerA.publicKey, amount: payoutA },
        { provider: providerB.publicKey, amount: payoutB },
      ],
    }]).accounts({
      admin: payer.publicKey,
      userAccount: userAccountPda,
      mint,
      userDepositTokenAccount: userDepositAta,
      feeCollectorTokenAccount: feeCollectorAta,
      tokenProgram: TOKEN_PROGRAM_ID,
    }).remainingAccounts([
      { pubkey: providerAtaA, isWritable: true, isSigner: false },
      { pubkey: providerAtaB, isWritable: true, isSigner: false },
    ]).rpc();

    // Verify Balances
    const feeCollected = (totalPayout.toNumber() * 2) / 100; // 2% of 3M = 60,000
    const balFee = await connection.getTokenAccountBalance(feeCollectorAta);
    expect(balFee.value.amount).to.equal(feeCollected.toString());

    const balA = await connection.getTokenAccountBalance(providerAtaA);
    expect(balA.value.amount).to.equal((payoutA.toNumber() * 0.98).toString());

    const acc = await program.account.userAccount.fetch(userAccountPda);
    // Initial 10M - 3M = 7M
    expect(acc.creditedAmount.toNumber()).to.equal(7_000_000);
    // Locked 3M (after Step 3b) - 3M (paid now) = 0
    expect(acc.lockedAmount.toNumber()).to.equal(0);

    console.log(`✅ Batch Payout Success!`);
    console.log(`💰 Platform Fee (2%) Collected: ${balFee.value.uiAmount} tokens`);
    console.log(`💰 Provider A (98%) Received: ${balA.value.uiAmount} tokens`);
  });
});
