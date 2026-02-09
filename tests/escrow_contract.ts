import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, SystemProgram } from "@solana/web3.js";
import {
  getAssociatedTokenAddress,
  getOrCreateAssociatedTokenAccount,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { EscrowContract } from "../target/types/escrow_contract";

describe("Lock payment", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace
    .EscrowContract as Program<EscrowContract>;

  const mint = new PublicKey(
    "Bq6duLAAY1i1xHSHorpHiHxjwGLbR9L1H3M4LEJJGSrS"
  );

  // Provider (receiver on release)
  const providerPubkey = new PublicKey(
    "36okfcgrtTh4XmbLKKqHxvocj5mArF78tjnsEiz5h95u"
  );

  it("Locks 10,000 SPL tokens into escrow", async () => {
    const user = provider.wallet.publicKey;

    // PDA
    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("escrow"), user.toBuffer()],
      program.programId
    );

    // User ATA
    const userTokenAccount = await getAssociatedTokenAddress(
      mint,
      user
    );

    // Escrow ATA (owned by PDA)
    const escrowTokenAccount = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      provider.wallet.payer,
      mint,
      escrowPda,
      true // allow owner off curve (PDA)
    );

    const amount = new anchor.BN(10_000 * 10 ** 6); // 10,000 tokens

    const tx = await program.methods
      .lockPayment(amount)
      .accounts({
        escrow: escrowPda,
        user,
        provider: providerPubkey,
        userTokenAccount,
        escrowTokenAccount: escrowTokenAccount.address,
        systemProgram: SystemProgram.programId,
        tokenProgram: TOKEN_PROGRAM_ID,
        rent: anchor.web3.SYSVAR_RENT_PUBKEY,
      })
      .rpc();

    console.log("Lock tx signature:", tx);
    console.log("Escrow PDA:", escrowPda.toBase58());
    console.log("Locked amount:", amount.toString());
  });
});
